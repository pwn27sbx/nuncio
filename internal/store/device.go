// Package store holds nuncio's persistence: a pure-Go SQLite database for
// everything that is not a secret, and the Secret Service for the WhatsApp
// device's key material.
package store

import (
	"context"
	cryptorand "crypto/rand"
	"database/sql"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	mathrand "math/rand/v2"

	"github.com/google/uuid"
	"github.com/zalando/go-keyring"
	"go.mau.fi/whatsmeow/proto/waAdv"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
	"google.golang.org/protobuf/proto"
)

// KeyringService owns nuncio's device keys in the Secret Service.
const KeyringService = "nuncio"

// The keyring items, one per piece of key material. Only private material is
// stored: the public halves are derived from it.
const (
	itemNoiseKey     = "device.noise_key"
	itemIdentityKey  = "device.identity_key"
	itemSignedPreKey = "device.signed_pre_key"
	itemAdvSecretKey = "device.adv_secret_key"
)

// keyItems is the full set of keyring items, in a fixed order.
var keyItems = []string{itemNoiseKey, itemIdentityKey, itemSignedPreKey, itemAdvSecretKey}

var (
	// ErrNoDevice reports a store that holds no device yet.
	ErrNoDevice = errors.New("store: no device stored")
	// ErrKeyNotFound reports a keyring item that is not there.
	ErrKeyNotFound = errors.New("store: key not found in the keyring")
)

// Keyring is the slice of the Secret Service the device store needs.
type Keyring interface {
	Set(service, user, secret string) error
	Get(service, user string) (string, error)
	Delete(service, user string) error
}

// SecretService is the owner's real keyring, through go-keyring over D-Bus.
type SecretService struct{}

func (SecretService) Set(service, user, secret string) error {
	return keyring.Set(service, user, secret)
}

func (SecretService) Get(service, user string) (string, error) {
	secret, err := keyring.Get(service, user)
	if errors.Is(err, keyring.ErrNotFound) {
		return "", ErrKeyNotFound
	}
	return secret, err
}

func (SecretService) Delete(service, user string) error {
	err := keyring.Delete(service, user)
	if errors.Is(err, keyring.ErrNotFound) {
		return ErrKeyNotFound
	}
	return err
}

// DeviceStore keeps the whatsmeow device's key material in the Secret Service
// and its non-secret metadata in SQLite.
//
// The keys go to exactly one place: there is deliberately no fallback that
// writes them to the database file, so a machine without a usable keyring
// cannot end up with an unencrypted copy of them.
type DeviceStore struct {
	db      *sql.DB
	keyring Keyring
	service string
}

var _ wastore.DeviceContainer = (*DeviceStore)(nil)

const deviceSchema = `
CREATE TABLE IF NOT EXISTS nuncio_device (
	id     INTEGER PRIMARY KEY CHECK (id = 1),
	record TEXT NOT NULL
)`

// NewDeviceStore returns a store that keeps the device keys in the owner's
// keyring under KeyringService.
func NewDeviceStore(db *sql.DB, kr Keyring) (*DeviceStore, error) {
	return NewDeviceStoreWithService(db, kr, KeyringService)
}

// NewDeviceStoreWithService is NewDeviceStore with an explicit keyring service
// name, so tests never touch the owner's keys.
func NewDeviceStoreWithService(db *sql.DB, kr Keyring, service string) (*DeviceStore, error) {
	if db == nil {
		return nil, errors.New("store: no database")
	}
	if kr == nil {
		return nil, errors.New("store: no keyring: the device keys must go to the Secret Service")
	}
	if service == "" {
		return nil, errors.New("store: no keyring service name")
	}
	s := &DeviceStore{db: db, keyring: kr, service: service}
	if _, err := db.Exec(deviceSchema); err != nil {
		return nil, fmt.Errorf("store: create the device table: %w", err)
	}
	return s, nil
}

// deviceRecord is every part of the device that is not key material. The
// WhatsApp account signature and the public keys in it are public by design.
type deviceRecord struct {
	JID                   string `json:"jid"`
	LID                   string `json:"lid"`
	RegistrationID        uint32 `json:"registration_id"`
	SignedPreKeyID        uint32 `json:"signed_pre_key_id"`
	Platform              string `json:"platform"`
	BusinessName          string `json:"business_name"`
	PushName              string `json:"push_name"`
	LIDMigrationTimestamp int64  `json:"lid_migration_timestamp"`
	CompanionMetaNonce    string `json:"companion_meta_nonce"`
	FacebookUUID          string `json:"facebook_uuid,omitempty"`
	Account               []byte `json:"account,omitempty"`
}

// PutDevice implements the DeviceContainer half of whatsmeow's device storage.
// It is called by Device.Save.
func (s *DeviceStore) PutDevice(ctx context.Context, device *wastore.Device) error {
	if device == nil {
		return errors.New("store: no device")
	}
	if err := s.putKeys(device); err != nil {
		return err
	}
	record, err := s.recordOf(device)
	if err != nil {
		return err
	}
	if _, err := s.db.ExecContext(ctx, `
		INSERT INTO nuncio_device (id, record) VALUES (1, ?)
		ON CONFLICT(id) DO UPDATE SET record = excluded.record`, record); err != nil {
		return fmt.Errorf("store: save the device record: %w", err)
	}
	return nil
}

// putKeys writes the key material to the Secret Service. The metadata row is
// written only after this succeeds, so a keyring failure leaves no half-saved
// device behind.
func (s *DeviceStore) putKeys(device *wastore.Device) error {
	switch {
	case device.NoiseKey == nil || device.NoiseKey.Priv == nil:
		return errors.New("store: the device has no noise key")
	case device.IdentityKey == nil || device.IdentityKey.Priv == nil:
		return errors.New("store: the device has no identity key")
	case device.SignedPreKey == nil || device.SignedPreKey.Priv == nil:
		return errors.New("store: the device has no signed pre-key")
	case device.SignedPreKey.Signature == nil:
		return errors.New("store: the device's signed pre-key has no signature")
	case len(device.AdvSecretKey) == 0:
		return errors.New("store: the device has no advertisement secret")
	}

	items := []struct {
		name  string
		value []byte
	}{
		{itemNoiseKey, device.NoiseKey.Priv[:]},
		{itemIdentityKey, device.IdentityKey.Priv[:]},
		{itemSignedPreKey, append(append([]byte{}, device.SignedPreKey.Priv[:]...), device.SignedPreKey.Signature[:]...)},
		{itemAdvSecretKey, device.AdvSecretKey},
	}
	for _, item := range items {
		secret := base64.StdEncoding.EncodeToString(item.value)
		if err := s.keyring.Set(s.service, item.name, secret); err != nil {
			return fmt.Errorf("store: write %s to the keyring (service %s): %w", item.name, s.service, err)
		}
	}
	return nil
}

func (s *DeviceStore) recordOf(device *wastore.Device) (string, error) {
	rec := deviceRecord{
		RegistrationID:        device.RegistrationID,
		Platform:              device.Platform,
		BusinessName:          device.BusinessName,
		PushName:              device.PushName,
		LIDMigrationTimestamp: device.LIDMigrationTimestamp,
		CompanionMetaNonce:    device.CompanionMetaNonce,
		SignedPreKeyID:        device.SignedPreKey.KeyID,
	}
	if device.ID != nil {
		rec.JID = device.ID.String()
	}
	if !device.LID.IsEmpty() {
		rec.LID = device.LID.String()
	}
	if device.FacebookUUID != uuid.Nil {
		rec.FacebookUUID = device.FacebookUUID.String()
	}
	if device.Account != nil {
		account, err := proto.Marshal(device.Account)
		if err != nil {
			return "", fmt.Errorf("store: marshal the account signature: %w", err)
		}
		rec.Account = account
	}
	record, err := json.Marshal(rec)
	if err != nil {
		return "", fmt.Errorf("store: marshal the device record: %w", err)
	}
	return string(record), nil
}

// errStoresNotWired marks the session-specific stores that arrive with the
// whatsmeow integration in M1.
var errStoresNotWired = errors.New("nuncio: the WhatsApp session stores land with M1")

// LoadDevice reads the device back: metadata from SQLite, key material from
// the Secret Service. It returns ErrNoDevice when nothing is stored yet.
func (s *DeviceStore) LoadDevice(ctx context.Context) (*wastore.Device, error) {
	var raw string
	err := s.db.QueryRowContext(ctx, `SELECT record FROM nuncio_device WHERE id = 1`).Scan(&raw)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNoDevice
	}
	if err != nil {
		return nil, fmt.Errorf("store: read the device record: %w", err)
	}

	var rec deviceRecord
	if err := json.Unmarshal([]byte(raw), &rec); err != nil {
		return nil, fmt.Errorf("store: parse the device record: %w", err)
	}

	noisePriv, err := s.privateKey(itemNoiseKey)
	if err != nil {
		return nil, err
	}
	identityPriv, err := s.privateKey(itemIdentityKey)
	if err != nil {
		return nil, err
	}
	preKey, err := s.signedPreKey(rec.SignedPreKeyID)
	if err != nil {
		return nil, err
	}
	advSecret, err := s.secret(itemAdvSecretKey)
	if err != nil {
		return nil, err
	}

	device := &wastore.Device{
		NoiseKey:              keys.NewKeyPairFromPrivateKey(noisePriv),
		IdentityKey:           keys.NewKeyPairFromPrivateKey(identityPriv),
		SignedPreKey:          preKey,
		AdvSecretKey:          advSecret,
		RegistrationID:        rec.RegistrationID,
		Platform:              rec.Platform,
		BusinessName:          rec.BusinessName,
		PushName:              rec.PushName,
		LIDMigrationTimestamp: rec.LIDMigrationTimestamp,
		CompanionMetaNonce:    rec.CompanionMetaNonce,
	}
	if rec.JID != "" {
		jid, err := types.ParseJID(rec.JID)
		if err != nil {
			return nil, fmt.Errorf("store: parse the stored JID %q: %w", rec.JID, err)
		}
		device.ID = &jid
	}
	if rec.LID != "" {
		lid, err := types.ParseJID(rec.LID)
		if err != nil {
			return nil, fmt.Errorf("store: parse the stored LID %q: %w", rec.LID, err)
		}
		device.LID = lid
	}
	if rec.FacebookUUID != "" {
		id, err := uuid.Parse(rec.FacebookUUID)
		if err != nil {
			return nil, fmt.Errorf("store: parse the stored Facebook UUID %q: %w", rec.FacebookUUID, err)
		}
		device.FacebookUUID = id
	}
	if len(rec.Account) > 0 {
		var account waAdv.ADVSignedDeviceIdentity
		if err := proto.Unmarshal(rec.Account, &account); err != nil {
			return nil, fmt.Errorf("store: parse the account signature: %w", err)
		}
		device.Account = &account
	}

	device.SetAllStores(&wastore.NoopStore{Error: errStoresNotWired})
	device.Container = s
	device.Initialized = true
	return device, nil
}

// DeleteDevice implements the DeviceContainer half of whatsmeow's device
// storage. It is called by Device.Delete.
func (s *DeviceStore) DeleteDevice(ctx context.Context, device *wastore.Device) error {
	for _, name := range keyItems {
		switch err := s.keyring.Delete(s.service, name); {
		case err == nil, errors.Is(err, ErrKeyNotFound):
		default:
			return fmt.Errorf("store: delete %s from the keyring (service %s): %w", name, s.service, err)
		}
	}
	if _, err := s.db.ExecContext(ctx, `DELETE FROM nuncio_device`); err != nil {
		return fmt.Errorf("store: delete the device record: %w", err)
	}
	return nil
}

// privateKey reads a 32-byte private key from the keyring.
func (s *DeviceStore) privateKey(name string) ([32]byte, error) {
	secret, err := s.secret(name)
	if err != nil {
		return [32]byte{}, err
	}
	if len(secret) != 32 {
		return [32]byte{}, fmt.Errorf("store: %s holds %d bytes, want a 32-byte private key", name, len(secret))
	}
	return [32]byte(secret), nil
}

// signedPreKey reads the stored private key and signature of the signed
// pre-key. Its key id is not key material and lives in the SQLite record.
func (s *DeviceStore) signedPreKey(keyID uint32) (*keys.PreKey, error) {
	secret, err := s.secret(itemSignedPreKey)
	if err != nil {
		return nil, err
	}
	if len(secret) != 96 {
		return nil, fmt.Errorf("store: %s holds %d bytes, want a 32-byte key and a 64-byte signature", itemSignedPreKey, len(secret))
	}
	preKey := &keys.PreKey{
		KeyPair: *keys.NewKeyPairFromPrivateKey([32]byte(secret[:32])),
		KeyID:   keyID,
	}
	signature := [64]byte(secret[32:])
	preKey.Signature = &signature
	return preKey, nil
}

// secret reads and base64-decodes one keyring item.
func (s *DeviceStore) secret(name string) ([]byte, error) {
	stored, err := s.keyring.Get(s.service, name)
	switch {
	case errors.Is(err, ErrKeyNotFound):
		return nil, fmt.Errorf("store: %w: read %s from the keyring (service %s)", err, name, s.service)
	case err != nil:
		return nil, fmt.Errorf("store: read %s from the keyring (service %s): %w", name, s.service, err)
	}
	secret, err := base64.StdEncoding.DecodeString(stored)
	if err != nil {
		return nil, fmt.Errorf("store: %s in the keyring is not valid base64: %w", name, err)
	}
	return secret, nil
}

// NewDevice returns an unpaired device with freshly generated keys, mirroring what whatsmeow's own
// sqlstore container does. Nothing is persisted until PutDevice runs, which the pairing flow does
// once a link succeeds — a device that was never linked leaves no trace.
func (s *DeviceStore) NewDevice() *wastore.Device {
	device := &wastore.Device{
		Container:      s,
		NoiseKey:       keys.NewKeyPair(),
		IdentityKey:    keys.NewKeyPair(),
		RegistrationID: mathrand.Uint32(),
		AdvSecretKey:   randomBytes(32),
	}
	device.SignedPreKey = device.IdentityKey.CreateSignedPreKey(1)
	return device
}

func randomBytes(n int) []byte {
	buf := make([]byte, n)
	if _, err := cryptorand.Read(buf); err != nil {
		// crypto/rand.Read only fails if the kernel has no entropy source; there is nothing
		// sensible to continue with, and a silently weak key would be worse than a crash.
		panic(fmt.Sprintf("store: no randomness available: %v", err))
	}
	return buf
}
