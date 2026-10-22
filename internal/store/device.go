// Package store holds nuncio's persistence: a pure-Go SQLite database, and the sealed device
// store (DeviceStore, sealedStore) that keeps the private key material of the WhatsApp device and
// its Signal state in it sealed under a data key held in the Secret Service (ADR 0003).
package store

import (
	"context"
	cryptorand "crypto/rand"
	"database/sql"
	"errors"
	"fmt"
	mathrand "math/rand/v2"

	"github.com/google/uuid"
	"github.com/zalando/go-keyring"
	"go.mau.fi/util/dbutil"
	"go.mau.fi/whatsmeow/proto/waAdv"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/store/sqlstore/upgrades"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
	waLog "go.mau.fi/whatsmeow/util/log"
)

// KeyringService owns nuncio's items in the Secret Service.
const KeyringService = "nuncio"

var (
	// ErrNoDevice reports a store that holds no device yet.
	ErrNoDevice = errors.New("store: no device stored")
	// ErrKeyNotFound reports a keyring item that is not there.
	ErrKeyNotFound = errors.New("store: key not found in the keyring")
	// ErrDeviceNotSealed reports a device row whose private keys are not in nuncio_device: a
	// database written by whatsmeow's own store that was not migrated yet.
	ErrDeviceNotSealed = errors.New("store: the stored device has no sealed keys")
)

// Keyring is the slice of the Secret Service nuncio needs.
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

// DeviceStore is whatsmeow's device container over nuncio.db with the private material sealed
// under the data key. The device's private keys live sealed in nuncio_device; whatsmeow_device
// keeps a stub row (the JID and the public fields, the private columns zeroed) so the foreign
// keys of every Signal table, and their ON DELETE CASCADE, keep working. The Signal state goes
// through sealedStore.
type DeviceStore struct {
	db        *dbutil.Database
	container *sqlstore.Container
	sealer    *Sealer
}

var _ wastore.DeviceContainer = (*DeviceStore)(nil)

// deviceSchema holds the sealed private keys of each device. A row goes with its
// whatsmeow_device row.
const deviceSchema = `
CREATE TABLE IF NOT EXISTS nuncio_device (
	jid    TEXT PRIMARY KEY,
	sealed BLOB NOT NULL,
	FOREIGN KEY (jid) REFERENCES whatsmeow_device(jid) ON DELETE CASCADE
)`

// The sealed device record: noise | identity | signed pre-key (32 bytes each) | ADV secret.
const devicePrivSize = 32

// zeroKey fills the private columns of the whatsmeow_device stub row. Their CHECKs want 32 bytes.
var zeroKey = make([]byte, devicePrivSize)

// NewDeviceStore prepares whatsmeow's tables and nuncio's sealed tables in db (opened with Open:
// foreign keys on) and returns a container that seals with sealer.
func NewDeviceStore(ctx context.Context, db *sql.DB, sealer *Sealer) (*DeviceStore, error) {
	if db == nil {
		return nil, errors.New("store: no database")
	}
	if sealer == nil {
		return nil, errors.New("store: no sealer: the device keys must be sealed under the data key")
	}
	wrapped, err := dbutil.NewWithDB(db, "sqlite3")
	if err != nil {
		return nil, fmt.Errorf("store: wrap the database: %w", err)
	}
	wrapped.UpgradeTable = upgrades.Table
	wrapped.VersionTable = "whatsmeow_version"
	// The container and the sealed store share this one wrapper: a transaction whatsmeow opens
	// (DoDecryptionTxn) lives in the context under this wrapper's key, so the sealed store's
	// queries join it instead of waiting for the only connection.
	container := sqlstore.NewWithWrappedDB(wrapped, waLog.Noop)
	if err := container.Upgrade(ctx); err != nil {
		return nil, fmt.Errorf("store: prepare the whatsmeow tables: %w", err)
	}
	for _, schema := range []string{deviceSchema, preKeySchema} {
		if _, err := wrapped.Exec(ctx, schema); err != nil {
			return nil, fmt.Errorf("store: create the sealed tables: %w", err)
		}
	}
	return &DeviceStore{db: wrapped, container: container, sealer: sealer}, nil
}

func deviceAAD(jid string) []byte { return sealAAD("nuncio_device", "sealed", jid, jid) }

// sealDeviceKeys seals the device record (noise | identity | signed pre-key | ADV secret) for
// nuncio_device. PutDevice and the migration (migrate_t17.go) both seal through it.
func sealDeviceKeys(sealer *Sealer, jid string, noise, identity, signedPreKey, advSecret []byte) []byte {
	private := make([]byte, 0, 3*devicePrivSize+len(advSecret))
	private = append(private, noise...)
	private = append(private, identity...)
	private = append(private, signedPreKey...)
	private = append(private, advSecret...)
	defer clear(private)
	return sealer.Seal(deviceAAD(jid), private)
}

const putDeviceStubQuery = `
	INSERT INTO whatsmeow_device (jid, lid, registration_id, noise_key, identity_key,
	                              signed_pre_key, signed_pre_key_id, signed_pre_key_sig,
	                              adv_key, adv_details, adv_account_sig, adv_account_sig_key, adv_device_sig,
	                              platform, business_name, push_name, facebook_uuid, lid_migration_ts, companion_meta_nonce)
	VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
	ON CONFLICT (jid) DO UPDATE
		SET lid=excluded.lid,
			platform=excluded.platform,
			business_name=excluded.business_name,
			push_name=excluded.push_name,
			lid_migration_ts=excluded.lid_migration_ts,
			companion_meta_nonce=excluded.companion_meta_nonce
`

// putDeviceSealedQuery keeps the first keys written, like whatsmeow's container, which never
// updates the key columns of an existing device.
const putDeviceSealedQuery = `INSERT INTO nuncio_device (jid, sealed) VALUES ($1, $2) ON CONFLICT (jid) DO NOTHING`

// PutDevice implements whatsmeow's DeviceContainer; Device.Save calls it. It writes the stub row
// and the sealed keys in one transaction, then wires the device's stores like whatsmeow's own
// container does.
func (s *DeviceStore) PutDevice(ctx context.Context, device *wastore.Device) error {
	if device == nil {
		return errors.New("store: no device")
	}
	if device.ID == nil {
		return sqlstore.ErrDeviceIDMustBeSet
	}
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
	case device.Account == nil:
		return errors.New("store: the device has no account signature")
	}
	jid := device.ID.String()
	sealed := sealDeviceKeys(s.sealer, jid, device.NoiseKey.Priv[:], device.IdentityKey.Priv[:],
		device.SignedPreKey.Priv[:], device.AdvSecretKey)

	err := s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		if _, err := s.db.Exec(ctx, putDeviceStubQuery,
			jid, device.LID, device.RegistrationID, zeroKey, zeroKey,
			zeroKey, device.SignedPreKey.KeyID, device.SignedPreKey.Signature[:],
			zeroKey, device.Account.Details, device.Account.AccountSignature, device.Account.AccountSignatureKey, device.Account.DeviceSignature,
			device.Platform, device.BusinessName, device.PushName, uuid.NullUUID{UUID: device.FacebookUUID, Valid: device.FacebookUUID != uuid.Nil},
			device.LIDMigrationTimestamp, device.CompanionMetaNonce,
		); err != nil {
			return fmt.Errorf("store: save the device row: %w", err)
		}
		if _, err := s.db.Exec(ctx, putDeviceSealedQuery, jid, sealed); err != nil {
			return fmt.Errorf("store: save the sealed device keys: %w", err)
		}
		return nil
	})
	if err != nil {
		return err
	}
	if !device.Initialized {
		s.initializeDevice(device)
	}
	return nil
}

// initializeDevice mirrors whatsmeow's container: the stores, the LID map and the container.
func (s *DeviceStore) initializeDevice(device *wastore.Device) {
	device.SetAllStores(newSealedStore(s.container, s.db, s.sealer, *device.ID))
	device.LIDs = s.container.LIDMap
	device.Container = s
	device.Initialized = true
}

const loadDeviceQuery = `
	SELECT d.jid, d.lid, d.registration_id, d.signed_pre_key_id, d.signed_pre_key_sig,
	       d.adv_details, d.adv_account_sig, d.adv_account_sig_key, d.adv_device_sig,
	       d.platform, d.business_name, d.push_name, d.facebook_uuid, d.lid_migration_ts, d.companion_meta_nonce,
	       n.sealed
	FROM whatsmeow_device d LEFT JOIN nuncio_device n ON n.jid = d.jid
	ORDER BY d.jid LIMIT 1
`

// LoadDevice reads the device back (the first by JID; nuncio links one), its keys opened from
// nuncio_device, with its stores wired.
// It returns ErrNoDevice when nothing is stored yet and ErrDeviceNotSealed for a device whose
// keys are not sealed.
func (s *DeviceStore) LoadDevice(ctx context.Context) (*wastore.Device, error) {
	var (
		jid, lid          sql.NullString
		preKeySig, sealed []byte
		account           waAdv.ADVSignedDeviceIdentity
		fbUUID            uuid.NullUUID
		preKeyID          uint32
		device            = &wastore.Device{Log: waLog.Noop}
	)
	err := s.db.QueryRow(ctx, loadDeviceQuery).Scan(
		&jid, &lid, &device.RegistrationID, &preKeyID, &preKeySig,
		&account.Details, &account.AccountSignature, &account.AccountSignatureKey, &account.DeviceSignature,
		&device.Platform, &device.BusinessName, &device.PushName, &fbUUID, &device.LIDMigrationTimestamp, &device.CompanionMetaNonce,
		&sealed)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrNoDevice
	}
	if err != nil {
		return nil, fmt.Errorf("store: read the device row: %w", err)
	}
	if sealed == nil {
		return nil, ErrDeviceNotSealed
	}
	id, err := types.ParseJID(jid.String)
	if err != nil {
		return nil, fmt.Errorf("store: parse the stored device JID: %w", err)
	}
	if lid.Valid && lid.String != "" {
		if device.LID, err = types.ParseJID(lid.String); err != nil {
			return nil, fmt.Errorf("store: parse the stored device LID: %w", err)
		}
	}
	if len(preKeySig) != 64 {
		return nil, sqlstore.ErrInvalidLength
	}
	private, err := s.sealer.Open(deviceAAD(id.String()), sealed)
	if err != nil {
		return nil, sealedReadError("nuncio_device", err)
	}
	defer clear(private)
	if len(private) <= 3*devicePrivSize {
		return nil, fmt.Errorf("store: the sealed device keys hold %d bytes, want more than %d", len(private), 3*devicePrivSize)
	}

	device.ID = &id
	device.NoiseKey = keys.NewKeyPairFromPrivateKey([32]byte(private[0:32]))
	device.IdentityKey = keys.NewKeyPairFromPrivateKey([32]byte(private[32:64]))
	device.SignedPreKey = &keys.PreKey{
		KeyPair:   *keys.NewKeyPairFromPrivateKey([32]byte(private[64:96])),
		KeyID:     preKeyID,
		Signature: (*[64]byte)(preKeySig),
	}
	device.AdvSecretKey = append([]byte(nil), private[96:]...)
	device.Account = &account
	device.FacebookUUID = fbUUID.UUID
	s.initializeDevice(device)
	return device, nil
}

// DeleteDevice implements whatsmeow's DeviceContainer; Device.Delete calls it. Deleting the
// whatsmeow_device row cascades to nuncio_device, nuncio_pre_keys and every Signal table with a
// foreign key to it. whatsmeow_privacy_tokens has none, so its rows for the device go explicitly.
func (s *DeviceStore) DeleteDevice(ctx context.Context, device *wastore.Device) error {
	if device == nil || device.ID == nil {
		return sqlstore.ErrDeviceIDMustBeSet
	}
	jid := device.ID.String()
	return s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		if _, err := s.db.Exec(ctx, `DELETE FROM whatsmeow_privacy_tokens WHERE our_jid=$1`, jid); err != nil {
			return fmt.Errorf("store: delete the device's privacy tokens: %w", err)
		}
		if _, err := s.db.Exec(ctx, `DELETE FROM whatsmeow_device WHERE jid=$1`, jid); err != nil {
			return fmt.Errorf("store: delete the device: %w", err)
		}
		return nil
	})
}

// NewDevice returns an unpaired device with freshly generated keys, mirroring what whatsmeow's own
// sqlstore container does. Nothing is persisted until PutDevice runs, which the pairing flow does
// once a link succeeds — a device that was never linked leaves no trace.
func (s *DeviceStore) NewDevice() *wastore.Device {
	device := &wastore.Device{
		Log:            waLog.Noop,
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
