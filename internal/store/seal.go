package store

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"database/sql"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
)

// The data key seals the private key material whatsmeow keeps in SQLite (ADR 0003). It lives in
// the Secret Service under KeyringService, base64 of 32 random bytes, and never in a file.
const itemDataKey = "store.data_key"

// dataKeySize is the AES-256 key length.
const dataKeySize = 32

// The sealed format: sealVersion | nonce (12) | ciphertext | tag (16).
const (
	sealVersion   = 0x01
	sealNonceSize = 12
	sealTagSize   = 16
)

// sealLabel opens every AAD, so a sealed value cannot be replayed under another scheme.
const sealLabel = "nuncio/seal/v1"

var (
	// ErrDataKeyMissing reports a keyring that holds no data key while the caller did not ask
	// for one to be created.
	ErrDataKeyMissing = errors.New("store: the data key is not in the keyring")
	// ErrKeyringUnavailable reports a keyring that could not be read or written, or that did not
	// keep what was written to it. It wraps the keyring's own error when there is one.
	ErrKeyringUnavailable = errors.New("store: the keyring is unavailable")
	// ErrDataKeyInvalid reports a data key item that is not base64 of 32 bytes. It is never
	// replaced: a new key would make everything sealed under the old one unreadable.
	ErrDataKeyInvalid = errors.New("store: the data key in the keyring is not valid")
	// ErrUnseal reports a sealed value that cannot be opened: unknown format, too short, another
	// key or another place (AAD), or altered bytes.
	ErrUnseal = errors.New("store: cannot open a sealed value")
	// ErrWrongDataKey reports a database whose check row was sealed under another data key.
	ErrWrongDataKey = errors.New("store: the data key does not match this database")
	// ErrSealCheckMissing reports a database without a check row. The caller decides what that
	// means: a fresh database gets one with WriteSealCheck, a database that already holds sealed
	// values must not be written.
	ErrSealCheckMissing = errors.New("store: the database has no seal check row")
)

// LoadDataKey returns the data key from the keyring. When the item is absent and create is
// true it generates a new key and keeps it with StoreDataKey, so a key that appeared meanwhile is
// never replaced: that key is returned instead. A new key is returned only when the keyring
// returns exactly what was written. When the item is absent and create is false it returns
// ErrDataKeyMissing and writes nothing.
//
// The keyring has no compare-and-swap: two callers that both find no key can both write one, the
// later write winning. A caller that creates the key under a lock every other creator also holds
// (the migration holds SQLite's write lock, MigrateToSealed) is never overtaken.
//
// Errors never carry the key or the stored item.
func LoadDataKey(kr Keyring, create bool) ([]byte, error) {
	if kr == nil {
		return nil, fmt.Errorf("%w: no keyring", ErrKeyringUnavailable)
	}
	key, err := readDataKey(kr)
	switch {
	case err == nil:
		return key, nil
	case !errors.Is(err, ErrDataKeyMissing):
		return nil, err
	case !create:
		return nil, err
	}

	key = randomBytes(dataKeySize)
	switch err := StoreDataKey(kr, key); {
	case err == nil:
		return key, nil
	case errors.Is(err, ErrDataKeyExists):
		clear(key)
		return readDataKey(kr)
	default:
		clear(key)
		return nil, err
	}
}

// writeDataKey writes key as the data key item and reads it back; it fails unless the keyring
// returns exactly what was written.
func writeDataKey(kr Keyring, key []byte) error {
	if err := kr.Set(KeyringService, itemDataKey, base64.StdEncoding.EncodeToString(key)); err != nil {
		return fmt.Errorf("%w: write %s (service %s): %w", ErrKeyringUnavailable, itemDataKey, KeyringService, err)
	}
	back, err := readDataKey(kr)
	if errors.Is(err, ErrDataKeyMissing) || errors.Is(err, ErrDataKeyInvalid) {
		return fmt.Errorf("%w: %s did not read back after the write (service %s)", ErrKeyringUnavailable, itemDataKey, KeyringService)
	}
	if err != nil {
		return err
	}
	defer clear(back)
	if !bytes.Equal(back, key) {
		return fmt.Errorf("%w: %s read back differs from what was written (service %s)", ErrKeyringUnavailable, itemDataKey, KeyringService)
	}
	return nil
}

// NewDataKey returns a fresh random data key. It is written nowhere: StoreDataKey keeps it.
func NewDataKey() []byte { return randomBytes(dataKeySize) }

// ErrDataKeyExists reports a keyring that already holds another data key. StoreDataKey never
// replaces it: whatever is sealed under it would become unreadable.
var ErrDataKeyExists = errors.New("store: the keyring already holds another data key")

// StoreDataKey keeps key (from NewDataKey) as the data key when the keyring holds none yet,
// and reads it back. A keyring that already holds key is left as it is; one that holds another
// key fails with ErrDataKeyExists and is not written. `nuncio link` calls it on the first save of
// a newly paired device, so a link that never pairs leaves no key behind.
//
// Errors never carry the key or the stored item.
func StoreDataKey(kr Keyring, key []byte) error {
	if kr == nil {
		return fmt.Errorf("%w: no keyring", ErrKeyringUnavailable)
	}
	if len(key) != dataKeySize {
		return fmt.Errorf("store: the data key is %d bytes, want %d", len(key), dataKeySize)
	}
	current, err := readDataKey(kr)
	switch {
	case err == nil:
		defer clear(current)
		if !bytes.Equal(current, key) {
			return ErrDataKeyExists
		}
		return nil
	case errors.Is(err, ErrDataKeyMissing):
		return writeDataKey(kr, key)
	default:
		return err
	}
}

// ForgetDataKey deletes the data key item, but only while it still holds key: it undoes a
// StoreDataKey whose save then failed, and never deletes a key another process wrote meanwhile.
func ForgetDataKey(kr Keyring, key []byte) error {
	if kr == nil {
		return fmt.Errorf("%w: no keyring", ErrKeyringUnavailable)
	}
	current, err := readDataKey(kr)
	if errors.Is(err, ErrDataKeyMissing) {
		return nil
	}
	if err != nil {
		return err
	}
	defer clear(current)
	if !bytes.Equal(current, key) {
		return nil
	}
	if err := kr.Delete(KeyringService, itemDataKey); err != nil && !errors.Is(err, ErrKeyNotFound) {
		return fmt.Errorf("%w: delete %s (service %s): %w", ErrKeyringUnavailable, itemDataKey, KeyringService, err)
	}
	return nil
}

// readDataKey reads and decodes the data key item.
func readDataKey(kr Keyring) ([]byte, error) {
	stored, err := kr.Get(KeyringService, itemDataKey)
	switch {
	case errors.Is(err, ErrKeyNotFound):
		return nil, fmt.Errorf("%w (item %s, service %s)", ErrDataKeyMissing, itemDataKey, KeyringService)
	case err != nil:
		return nil, fmt.Errorf("%w: read %s (service %s): %w", ErrKeyringUnavailable, itemDataKey, KeyringService, err)
	}
	key, err := base64.StdEncoding.DecodeString(stored)
	if err != nil {
		// The decoder's error quotes an input offset, never the input; still, keep it out.
		return nil, fmt.Errorf("%w: %s is not base64 (service %s)", ErrDataKeyInvalid, itemDataKey, KeyringService)
	}
	if len(key) != dataKeySize {
		return nil, fmt.Errorf("%w: %s holds %d bytes, want %d (service %s)", ErrDataKeyInvalid, itemDataKey, len(key), dataKeySize, KeyringService)
	}
	return key, nil
}

// Sealer seals and opens values with AES-256-GCM under the data key.
type Sealer struct {
	aead cipher.AEAD
}

// NewSealer returns a sealer for a 32-byte data key.
func NewSealer(key []byte) (*Sealer, error) {
	if len(key) != dataKeySize {
		return nil, fmt.Errorf("store: the data key is %d bytes, want %d", len(key), dataKeySize)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, fmt.Errorf("store: prepare the data key: %w", err)
	}
	aead, err := cipher.NewGCMWithRandomNonce(block)
	if err != nil {
		return nil, fmt.Errorf("store: prepare the data key: %w", err)
	}
	return &Sealer{aead: aead}, nil
}

// Seal returns sealVersion | nonce | ciphertext | tag for plaintext, bound to aad. Every call
// draws a fresh random nonce.
func (s *Sealer) Seal(aad, plaintext []byte) []byte {
	out := make([]byte, 1, 1+s.aead.Overhead()+len(plaintext))
	out[0] = sealVersion
	return s.aead.Seal(out, nil, plaintext, aad)
}

// Open returns the plaintext of a value Seal produced under the same key and aad. Any other
// input fails with ErrUnseal; the error never carries the input, the key or the plaintext.
func (s *Sealer) Open(aad, sealed []byte) ([]byte, error) {
	if len(sealed) < 1+sealNonceSize+sealTagSize {
		return nil, fmt.Errorf("%w: %d bytes is shorter than the sealed format", ErrUnseal, len(sealed))
	}
	if sealed[0] != sealVersion {
		return nil, fmt.Errorf("%w: unknown format version", ErrUnseal)
	}
	plaintext, err := s.aead.Open(nil, nil, sealed[1:], aad)
	if err != nil {
		return nil, fmt.Errorf("%w: authentication failed (another key, another place, or altered bytes)", ErrUnseal)
	}
	return plaintext, nil
}

// sealAAD binds a sealed value to where it is stored: the table, the column, the account the
// database belongs to and the row's primary key. Every part is length-prefixed, so moving a
// boundary ("ab","c" against "a","bc") gives another AAD.
func sealAAD(table, column, ourJID string, pk ...string) []byte {
	parts := append([]string{sealLabel, table, column, ourJID}, pk...)
	size := 0
	for _, p := range parts {
		size += 4 + len(p)
	}
	out := make([]byte, 0, size)
	for _, p := range parts {
		out = binary.BigEndian.AppendUint32(out, uint32(len(p)))
		out = append(out, p...)
	}
	return out
}

// The key check: one row holding a constant sealed under the data key, written when a database
// is set up and opened before anything else is written, so a wrong key is caught before it can
// mix values sealed under two keys. The table belongs to the sealing layer, like nuncio_device
// belongs to the device store, so it is declared here and not in schema.go.
const sealMetaSchema = `
CREATE TABLE IF NOT EXISTS nuncio_seal_meta (
	id      INTEGER PRIMARY KEY CHECK (id = 1),
	version INTEGER NOT NULL,
	"check" BLOB NOT NULL
)`

// sealCheckPlaintext is the constant the check row seals. It is not secret.
const sealCheckPlaintext = "nuncio seal check"

func sealCheckAAD() []byte { return sealAAD("nuncio_seal_meta", "check", "") }

// WriteSealCheck gives a database its check row. When the row already exists it only verifies
// it, so the row is never replaced, and a wrong key fails with ErrWrongDataKey before anything
// is written.
func WriteSealCheck(ctx context.Context, db *sql.DB, s *Sealer) error {
	switch err := VerifySealCheck(ctx, db, s); {
	case err == nil:
		return nil
	case !errors.Is(err, ErrSealCheckMissing):
		return err
	}
	tx, err := db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("store: begin the seal check write: %w", err)
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.ExecContext(ctx, sealMetaSchema); err != nil {
		return fmt.Errorf("store: create the seal check table: %w", err)
	}
	if _, err := tx.ExecContext(ctx, `INSERT INTO nuncio_seal_meta (id, version, "check") VALUES (1, ?, ?)`,
		sealVersion, s.Seal(sealCheckAAD(), []byte(sealCheckPlaintext))); err != nil {
		return fmt.Errorf("store: write the seal check row: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("store: commit the seal check row: %w", err)
	}
	return nil
}

// VerifySealCheck opens the check row with s. It writes nothing, not even the table. It returns
// ErrSealCheckMissing when there is no row and ErrWrongDataKey when the row was sealed under
// another key.
func VerifySealCheck(ctx context.Context, db *sql.DB, s *Sealer) error {
	var tables int
	if err := db.QueryRowContext(ctx,
		`SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'nuncio_seal_meta'`).Scan(&tables); err != nil {
		return fmt.Errorf("store: look for the seal check table: %w", err)
	}
	if tables == 0 {
		return ErrSealCheckMissing
	}
	var version int64
	var check []byte
	err := db.QueryRowContext(ctx, `SELECT version, "check" FROM nuncio_seal_meta WHERE id = 1`).Scan(&version, &check)
	if errors.Is(err, sql.ErrNoRows) {
		return ErrSealCheckMissing
	}
	if err != nil {
		return fmt.Errorf("store: read the seal check row: %w", err)
	}
	if version != sealVersion {
		return fmt.Errorf("store: the seal check row has version %d, this build knows %d", version, sealVersion)
	}
	plaintext, err := s.Open(sealCheckAAD(), check)
	if err != nil || string(plaintext) != sealCheckPlaintext {
		return ErrWrongDataKey
	}
	return nil
}
