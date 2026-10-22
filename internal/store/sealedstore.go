package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"go.mau.fi/util/dbutil"
	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
)

// sealedStore is whatsmeow's *sqlstore.SQLStore with every private column sealed under the data
// key (ADR 0003, S6). It overrides each method that reads or writes a sealed column and leaves the
// rest, which only touch plain columns, to SQLStore. TestSQLStoreMethodSetIsPinned fails when a
// whatsmeow bump changes SQLStore's methods, so a new one cannot write key bytes in plain unseen.
//
// Every sealed value is bound (AAD) to its table, its column, our JID and the rest of its row's
// primary key; our JID is the our_jid (or jid) column of every primary key here. A row that does
// not open fails with an error that names the table only.
type sealedStore struct {
	*sqlstore.SQLStore
	// db is the wrapper the container uses, so these queries join a transaction whatsmeow
	// carries in the context (DoDecryptionTxn).
	db     *dbutil.Database
	sealer *Sealer
	ourJID string

	preKeyLock sync.Mutex

	migratedLock sync.Mutex
	migrated     map[string]struct{}
}

var _ wastore.AllSessionSpecificStores = (*sealedStore)(nil)

// errDevicesBelongToDeviceStore is what sealedStore answers for the *sqlstore.Container device
// methods it would otherwise promote: they read and write whatsmeow_device's private columns in
// plain, and the device is DeviceStore's to load, save and delete.
var errDevicesBelongToDeviceStore = errors.New("store: the device belongs to DeviceStore, not to a sealed session store")

func newSealedStore(c *sqlstore.Container, db *dbutil.Database, sealer *Sealer, jid types.JID) *sealedStore {
	return &sealedStore{
		SQLStore: sqlstore.NewSQLStore(c, jid),
		db:       db,
		sealer:   sealer,
		ourJID:   jid.String(),
		migrated: map[string]struct{}{},
	}
}

// The *sqlstore.Container methods that SQLStore embeds and sealedStore would promote. Each reads or
// writes whatsmeow_device, whose private columns the Container keeps in plain, so here each one
// fails. Close and Upgrade stay promoted: they touch no row (Close closes the database handle,
// Upgrade runs whatsmeow's schema upgrades). TestSealedStoreRefusesTheContainersDeviceMethods
// fails when a whatsmeow bump adds another.

func (s *sealedStore) PutDevice(context.Context, *wastore.Device) error {
	return errDevicesBelongToDeviceStore
}

func (s *sealedStore) DeleteDevice(context.Context, *wastore.Device) error {
	return errDevicesBelongToDeviceStore
}

func (s *sealedStore) GetDevice(context.Context, types.JID) (*wastore.Device, error) {
	return nil, errDevicesBelongToDeviceStore
}

func (s *sealedStore) GetAllDevices(context.Context) ([]*wastore.Device, error) {
	return nil, errDevicesBelongToDeviceStore
}

func (s *sealedStore) GetFirstDevice(context.Context) (*wastore.Device, error) {
	return nil, errDevicesBelongToDeviceStore
}

// NewDevice has no error to return, so it panics: a device made here would save through the
// Container, its keys in plain.
func (s *sealedStore) NewDevice() *wastore.Device {
	panic(errDevicesBelongToDeviceStore)
}

// The sealed tables and columns.
const (
	tSessions    = "whatsmeow_sessions"
	tSenderKeys  = "whatsmeow_sender_keys"
	tAppStateKey = "whatsmeow_app_state_sync_keys"
	tMsgSecrets  = "whatsmeow_message_secrets"
	tPrivacy     = "whatsmeow_privacy_tokens"
	tNCTSalt     = "whatsmeow_nct_salt"
	tEventBuffer = "whatsmeow_event_buffer"
	tRetryBuffer = "whatsmeow_retry_buffer"
	tPreKeys     = "nuncio_pre_keys"
)

// sealedReadError reports a sealed value that does not open. It names the table only: no key
// bytes, no ids.
func sealedReadError(table string, err error) error {
	return fmt.Errorf("store: a sealed value in %s does not open: %w", table, err)
}

func (s *sealedStore) seal(table, column string, value []byte, pk ...string) []byte {
	return s.sealer.Seal(sealAAD(table, column, s.ourJID, pk...), value)
}

// sealNullable keeps NULL as NULL, like the stock store does with a nil value: a nullable column
// stores NULL, and a NOT NULL one refuses the write with the stock store's constraint error.
func (s *sealedStore) sealNullable(table, column string, value []byte, pk ...string) []byte {
	if value == nil {
		return nil
	}
	return s.seal(table, column, value, pk...)
}

func (s *sealedStore) open(table, column string, sealed []byte, pk ...string) ([]byte, error) {
	if sealed == nil {
		return nil, nil
	}
	plain, err := s.sealer.Open(sealAAD(table, column, s.ourJID, pk...), sealed)
	if err != nil {
		return nil, sealedReadError(table, err)
	}
	return plain, nil
}

// Sessions.

const (
	getSessionQuery     = `SELECT session FROM whatsmeow_sessions WHERE our_jid=$1 AND their_id=$2`
	getManySessionQuery = `SELECT their_id, session FROM whatsmeow_sessions WHERE our_jid=$1 AND their_id IN (%s)`
	putSessionQuery     = `
		INSERT INTO whatsmeow_sessions (our_jid, their_id, session) VALUES ($1, $2, $3)
		ON CONFLICT (our_jid, their_id) DO UPDATE SET session=excluded.session
	`
)

func (s *sealedStore) GetSession(ctx context.Context, address string) ([]byte, error) {
	var sealed []byte
	err := s.db.QueryRow(ctx, getSessionQuery, s.ourJID, address).Scan(&sealed)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	return s.open(tSessions, "session", sealed, address)
}

func (s *sealedStore) GetManySessions(ctx context.Context, addresses []string) (map[string][]byte, error) {
	if len(addresses) == 0 {
		return nil, nil
	}
	args := make([]any, len(addresses)+1)
	placeholders := make([]string, len(addresses))
	args[0] = s.ourJID
	for i, addr := range addresses {
		args[i+1] = addr
		placeholders[i] = fmt.Sprintf("$%d", i+2)
	}
	rows, err := s.db.Query(ctx, fmt.Sprintf(getManySessionQuery, strings.Join(placeholders, ",")), args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	result := make(map[string][]byte, len(addresses))
	for _, addr := range addresses {
		result[addr] = nil
	}
	for rows.Next() {
		var addr string
		var sealed []byte
		if err := rows.Scan(&addr, &sealed); err != nil {
			return nil, err
		}
		if result[addr], err = s.open(tSessions, "session", sealed, addr); err != nil {
			return nil, err
		}
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return result, nil
}

func (s *sealedStore) PutSession(ctx context.Context, address string, session []byte) error {
	_, err := s.db.Exec(ctx, putSessionQuery, s.ourJID, address, s.sealNullable(tSessions, "session", session, address))
	return err
}

func (s *sealedStore) PutManySessions(ctx context.Context, sessions map[string][]byte) error {
	return s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		for addr, sess := range sessions {
			if err := s.PutSession(ctx, addr, sess); err != nil {
				return err
			}
		}
		return nil
	})
}

// MigratePNToLID moves the same rows as SQLStore's (sessions, identity keys and sender keys
// whose address user is the PN) to the LID address, in one transaction. SQLStore copies them in
// SQL; here each sealed value is opened under its PN row and sealed again under its LID row.
// Like SQLStore it runs once per PN; unlike it, a failed migration may be tried again.
func (s *sealedStore) MigratePNToLID(ctx context.Context, pn, lid types.JID) error {
	pnSignal, lidSignal := pn.SignalAddressUser(), lid.SignalAddressUser()
	s.migratedLock.Lock()
	_, done := s.migrated[pnSignal]
	s.migrated[pnSignal] = struct{}{}
	s.migratedLock.Unlock()
	if done {
		return nil
	}
	err := s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		if err := s.migrateSessions(ctx, pnSignal, lidSignal); err != nil {
			return err
		}
		if _, err := s.db.Exec(ctx, migrateIdentityKeysQuery, s.ourJID, pnSignal, lidSignal); err != nil {
			return fmt.Errorf("store: migrate identity keys: %w", err)
		}
		if _, err := s.db.Exec(ctx, `DELETE FROM whatsmeow_identity_keys WHERE our_jid=$1 AND their_id LIKE $2`, s.ourJID, pnSignal+":%"); err != nil {
			return fmt.Errorf("store: delete migrated identity keys: %w", err)
		}
		return s.migrateSenderKeys(ctx, pnSignal, lidSignal)
	})
	if err != nil {
		s.migratedLock.Lock()
		delete(s.migrated, pnSignal)
		s.migratedLock.Unlock()
		return err
	}
	return nil
}

// Identity keys are public: they move in SQL, as in SQLStore.
const migrateIdentityKeysQuery = `
	INSERT INTO whatsmeow_identity_keys (our_jid, their_id, identity)
	SELECT our_jid, replace(their_id, $2, $3), identity
	FROM whatsmeow_identity_keys
	WHERE our_jid=$1 AND their_id LIKE $2 || ':%'
	ON CONFLICT (our_jid, their_id) DO UPDATE SET identity=excluded.identity
`

func (s *sealedStore) migrateSessions(ctx context.Context, pnSignal, lidSignal string) error {
	type row struct {
		addr   string
		sealed []byte
	}
	var moved []row
	err := s.scanAll(ctx, func(rows dbutil.Rows) error {
		var r row
		if err := rows.Scan(&r.addr, &r.sealed); err != nil {
			return err
		}
		moved = append(moved, r)
		return nil
	}, `SELECT their_id, session FROM whatsmeow_sessions WHERE our_jid=$1 AND their_id LIKE $2 || ':%'`, s.ourJID, pnSignal)
	if err != nil {
		return fmt.Errorf("store: read the sessions to migrate: %w", err)
	}
	for _, r := range moved {
		plain, err := s.open(tSessions, "session", r.sealed, r.addr)
		if err != nil {
			return err
		}
		if err := s.PutSession(ctx, strings.ReplaceAll(r.addr, pnSignal, lidSignal), plain); err != nil {
			return fmt.Errorf("store: migrate a session: %w", err)
		}
	}
	if _, err := s.db.Exec(ctx, `DELETE FROM whatsmeow_sessions WHERE our_jid=$1 AND their_id LIKE $2`, s.ourJID, pnSignal+":%"); err != nil {
		return fmt.Errorf("store: delete migrated sessions: %w", err)
	}
	return nil
}

func (s *sealedStore) migrateSenderKeys(ctx context.Context, pnSignal, lidSignal string) error {
	type row struct {
		chat, sender string
		sealed       []byte
	}
	var moved []row
	err := s.scanAll(ctx, func(rows dbutil.Rows) error {
		var r row
		if err := rows.Scan(&r.chat, &r.sender, &r.sealed); err != nil {
			return err
		}
		moved = append(moved, r)
		return nil
	}, `SELECT chat_id, sender_id, sender_key FROM whatsmeow_sender_keys WHERE our_jid=$1 AND sender_id LIKE $2 || ':%'`, s.ourJID, pnSignal)
	if err != nil {
		return fmt.Errorf("store: read the sender keys to migrate: %w", err)
	}
	for _, r := range moved {
		plain, err := s.open(tSenderKeys, "sender_key", r.sealed, r.chat, r.sender)
		if err != nil {
			return err
		}
		if err := s.PutSenderKey(ctx, r.chat, strings.ReplaceAll(r.sender, pnSignal, lidSignal), plain); err != nil {
			return fmt.Errorf("store: migrate a sender key: %w", err)
		}
	}
	if _, err := s.db.Exec(ctx, `DELETE FROM whatsmeow_sender_keys WHERE our_jid=$1 AND sender_id LIKE $2`, s.ourJID, pnSignal+":%"); err != nil {
		return fmt.Errorf("store: delete migrated sender keys: %w", err)
	}
	return nil
}

// scanAll runs query and hands each row to scan, closing the rows before it returns, so the
// caller can write on the same connection afterwards.
func (s *sealedStore) scanAll(ctx context.Context, scan func(dbutil.Rows) error, query string, args ...any) error {
	rows, err := s.db.Query(ctx, query, args...)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		if err := scan(rows); err != nil {
			return err
		}
	}
	return rows.Err()
}

// Pre-keys live in nuncio_pre_keys, sealed, with SQLStore's semantics: ids count up from the
// highest stored one, GetOrGenPreKeys returns the oldest not yet uploaded keys first and tops
// them up with new ones, GenOnePreKey stores its key as uploaded.

const preKeySchema = `
CREATE TABLE IF NOT EXISTS nuncio_pre_keys (
	jid      TEXT    NOT NULL,
	key_id   INTEGER NOT NULL CHECK (key_id >= 0 AND key_id < 16777216),
	sealed   BLOB    NOT NULL,
	uploaded BOOLEAN NOT NULL,
	PRIMARY KEY (jid, key_id),
	FOREIGN KEY (jid) REFERENCES whatsmeow_device(jid) ON DELETE CASCADE
)`

// preKeyAAD is the primary-key part of a pre-key's AAD after our JID. The migration (migrate_t17.go)
// seals with it too.
func preKeyAAD(id uint32) string { return strconv.FormatUint(uint64(id), 10) }

func (s *sealedStore) genOnePreKey(ctx context.Context, id uint32, markUploaded bool) (*keys.PreKey, error) {
	key := keys.NewPreKey(id)
	_, err := s.db.Exec(ctx, `INSERT INTO nuncio_pre_keys (jid, key_id, sealed, uploaded) VALUES ($1, $2, $3, $4)`,
		s.ourJID, key.KeyID, s.seal(tPreKeys, "sealed", key.Priv[:], preKeyAAD(key.KeyID)), markUploaded)
	return key, err
}

func (s *sealedStore) getNextPreKeyID(ctx context.Context) (uint32, error) {
	var last sql.NullInt32
	if err := s.db.QueryRow(ctx, `SELECT MAX(key_id) FROM nuncio_pre_keys WHERE jid=$1`, s.ourJID).Scan(&last); err != nil {
		return 0, fmt.Errorf("store: query the next pre-key id: %w", err)
	}
	return uint32(last.Int32) + 1, nil
}

func (s *sealedStore) openPreKey(id uint32, sealed []byte) (*keys.PreKey, error) {
	priv, err := s.open(tPreKeys, "sealed", sealed, preKeyAAD(id))
	if err != nil {
		return nil, err
	}
	if len(priv) != 32 {
		return nil, sqlstore.ErrInvalidLength
	}
	defer clear(priv)
	return &keys.PreKey{KeyPair: *keys.NewKeyPairFromPrivateKey([32]byte(priv)), KeyID: id}, nil
}

func (s *sealedStore) GenOnePreKey(ctx context.Context) (*keys.PreKey, error) {
	s.preKeyLock.Lock()
	defer s.preKeyLock.Unlock()
	next, err := s.getNextPreKeyID(ctx)
	if err != nil {
		return nil, err
	}
	return s.genOnePreKey(ctx, next, true)
}

func (s *sealedStore) GetOrGenPreKeys(ctx context.Context, count uint32) ([]*keys.PreKey, error) {
	s.preKeyLock.Lock()
	defer s.preKeyLock.Unlock()

	var newKeys []*keys.PreKey
	err := s.scanAll(ctx, func(rows dbutil.Rows) error {
		var id uint32
		var sealed []byte
		if err := rows.Scan(&id, &sealed); err != nil {
			return err
		}
		key, err := s.openPreKey(id, sealed)
		if err != nil {
			return err
		}
		newKeys = append(newKeys, key)
		return nil
	}, `SELECT key_id, sealed FROM nuncio_pre_keys WHERE jid=$1 AND uploaded=false ORDER BY key_id LIMIT $2`, s.ourJID, count)
	if err != nil {
		return nil, fmt.Errorf("store: query existing pre-keys: %w", err)
	}
	already := uint32(len(newKeys))
	if count > already {
		next, err := s.getNextPreKeyID(ctx)
		if err != nil {
			return nil, err
		}
		newKeys = slices.Grow(newKeys, int(count)-len(newKeys))[:count]
		for i := already; i < count; i++ {
			if newKeys[i], err = s.genOnePreKey(ctx, next, false); err != nil {
				return nil, fmt.Errorf("store: generate a pre-key: %w", err)
			}
			next++
		}
	}
	return newKeys, nil
}

func (s *sealedStore) GetPreKey(ctx context.Context, id uint32) (*keys.PreKey, error) {
	var keyID uint32
	var sealed []byte
	err := s.db.QueryRow(ctx, `SELECT key_id, sealed FROM nuncio_pre_keys WHERE jid=$1 AND key_id=$2`, s.ourJID, id).Scan(&keyID, &sealed)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	return s.openPreKey(keyID, sealed)
}

func (s *sealedStore) RemovePreKey(ctx context.Context, id uint32) error {
	_, err := s.db.Exec(ctx, `DELETE FROM nuncio_pre_keys WHERE jid=$1 AND key_id=$2`, s.ourJID, id)
	return err
}

func (s *sealedStore) MarkPreKeysAsUploaded(ctx context.Context, upToID uint32) error {
	_, err := s.db.Exec(ctx, `UPDATE nuncio_pre_keys SET uploaded=true WHERE jid=$1 AND key_id<=$2`, s.ourJID, upToID)
	return err
}

func (s *sealedStore) UploadedPreKeyCount(ctx context.Context) (count int, err error) {
	err = s.db.QueryRow(ctx, `SELECT COUNT(*) FROM nuncio_pre_keys WHERE jid=$1 AND uploaded=true`, s.ourJID).Scan(&count)
	return
}

// Sender keys.

func (s *sealedStore) PutSenderKey(ctx context.Context, group, user string, session []byte) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO whatsmeow_sender_keys (our_jid, chat_id, sender_id, sender_key) VALUES ($1, $2, $3, $4)
		ON CONFLICT (our_jid, chat_id, sender_id) DO UPDATE SET sender_key=excluded.sender_key`,
		s.ourJID, group, user, s.sealNullable(tSenderKeys, "sender_key", session, group, user))
	return err
}

func (s *sealedStore) GetSenderKey(ctx context.Context, group, user string) ([]byte, error) {
	var sealed []byte
	err := s.db.QueryRow(ctx, `SELECT sender_key FROM whatsmeow_sender_keys WHERE our_jid=$1 AND chat_id=$2 AND sender_id=$3`,
		s.ourJID, group, user).Scan(&sealed)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	return s.open(tSenderKeys, "sender_key", sealed, group, user)
}

// App-state sync keys: the key data is sealed; the key id, timestamp and fingerprint stay plain.

func (s *sealedStore) PutAppStateSyncKey(ctx context.Context, id []byte, key wastore.AppStateSyncKey) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO whatsmeow_app_state_sync_keys (jid, key_id, key_data, timestamp, fingerprint) VALUES ($1, $2, $3, $4, $5)
		ON CONFLICT (jid, key_id) DO UPDATE
			SET key_data=excluded.key_data, timestamp=excluded.timestamp, fingerprint=excluded.fingerprint
			WHERE excluded.timestamp > whatsmeow_app_state_sync_keys.timestamp`,
		s.ourJID, id, s.seal(tAppStateKey, "key_data", key.Data, string(id)), key.Timestamp, key.Fingerprint)
	return err
}

func (s *sealedStore) scanAppStateSyncKey(scan func(...any) error) (*wastore.AppStateSyncKey, error) {
	var id, sealed []byte
	var item wastore.AppStateSyncKey
	if err := scan(&id, &sealed, &item.Timestamp, &item.Fingerprint); err != nil {
		return nil, err
	}
	data, err := s.open(tAppStateKey, "key_data", sealed, string(id))
	if err != nil {
		return nil, err
	}
	item.Data = data
	return &item, nil
}

func (s *sealedStore) GetAppStateSyncKey(ctx context.Context, id []byte) (*wastore.AppStateSyncKey, error) {
	key, err := s.scanAppStateSyncKey(s.db.QueryRow(ctx,
		`SELECT key_id, key_data, timestamp, fingerprint FROM whatsmeow_app_state_sync_keys WHERE jid=$1 AND key_id=$2`, s.ourJID, id).Scan)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return key, err
}

func (s *sealedStore) GetAllAppStateSyncKeys(ctx context.Context) ([]*wastore.AppStateSyncKey, error) {
	var all []*wastore.AppStateSyncKey
	err := s.scanAll(ctx, func(rows dbutil.Rows) error {
		key, err := s.scanAppStateSyncKey(rows.Scan)
		if err != nil {
			return err
		}
		all = append(all, key)
		return nil
	}, `SELECT key_id, key_data, timestamp, fingerprint FROM whatsmeow_app_state_sync_keys WHERE jid=$1 ORDER BY timestamp DESC`, s.ourJID)
	if err != nil {
		return nil, err
	}
	return all, nil
}

// Message secrets. The lookup is SQLStore's, which also finds a row stored under the other
// address of a PN/LID pair; it also returns the stored chat, so the AAD is the stored row's.

const getMsgSecretQuery = `
	SELECT key, chat_jid, sender_jid
	FROM whatsmeow_message_secrets
	WHERE our_jid=$1 AND (chat_jid=$2 OR chat_jid=(
		CASE
			WHEN $2 LIKE '%@lid'
				THEN (SELECT pn || '@s.whatsapp.net' FROM whatsmeow_lid_map WHERE lid=replace($2, '@lid', ''))
			WHEN $2 LIKE '%@s.whatsapp.net'
				THEN (SELECT lid || '@lid' FROM whatsmeow_lid_map WHERE pn=replace($2, '@s.whatsapp.net', ''))
		END
	)) AND message_id=$4 AND (sender_jid=$3 OR sender_jid=(
		CASE
			WHEN $3 LIKE '%@lid'
				THEN (SELECT pn || '@s.whatsapp.net' FROM whatsmeow_lid_map WHERE lid=replace($3, '@lid', ''))
			WHEN $3 LIKE '%@s.whatsapp.net'
				THEN (SELECT lid || '@lid' FROM whatsmeow_lid_map WHERE pn=replace($3, '@s.whatsapp.net', ''))
		END
	))
`

func (s *sealedStore) PutMessageSecrets(ctx context.Context, inserts []wastore.MessageSecretInsert) error {
	if len(inserts) == 0 {
		return nil
	}
	return s.db.DoTxn(ctx, nil, func(ctx context.Context) error {
		for _, insert := range inserts {
			if insert.Chat.IsEmpty() || insert.Sender.IsEmpty() || insert.ID == "" || len(insert.Secret) == 0 {
				continue
			}
			if err := s.PutMessageSecret(ctx, insert.Chat, insert.Sender, insert.ID, insert.Secret); err != nil {
				return err
			}
		}
		return nil
	})
}

func (s *sealedStore) PutMessageSecret(ctx context.Context, chat, sender types.JID, id types.MessageID, secret []byte) error {
	chatJID, senderJID := chat.ToNonAD(), sender.ToNonAD()
	_, err := s.db.Exec(ctx, `
		INSERT INTO whatsmeow_message_secrets (our_jid, chat_jid, sender_jid, message_id, key)
		VALUES ($1, $2, $3, $4, $5)
		ON CONFLICT (our_jid, chat_jid, sender_jid, message_id) DO NOTHING`,
		s.ourJID, chatJID, senderJID, id, s.seal(tMsgSecrets, "key", secret, chatJID.String(), senderJID.String(), id))
	return err
}

func (s *sealedStore) GetMessageSecret(ctx context.Context, chat, sender types.JID, id types.MessageID) ([]byte, types.JID, error) {
	var sealed []byte
	var storedChat, storedSender string
	err := s.db.QueryRow(ctx, getMsgSecretQuery, s.ourJID, chat.ToNonAD(), sender.ToNonAD(), id).Scan(&sealed, &storedChat, &storedSender)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, types.EmptyJID, nil
	} else if err != nil {
		return nil, types.EmptyJID, err
	}
	realSender, err := types.ParseJID(storedSender)
	if err != nil {
		return nil, types.EmptyJID, fmt.Errorf("store: parse a stored message secret's sender: %w", err)
	}
	secret, err := s.open(tMsgSecrets, "key", sealed, storedChat, storedSender, id)
	if err != nil {
		return nil, types.EmptyJID, err
	}
	return secret, realSender, nil
}

// Privacy tokens. The lookup is SQLStore's and returns the stored contact for the AAD.

const (
	putPrivacyTokensQuery = `
		INSERT INTO whatsmeow_privacy_tokens (our_jid, their_jid, token, timestamp, sender_timestamp)
		VALUES ($1, $2, $3, $4, $5)
		ON CONFLICT (our_jid, their_jid) DO UPDATE SET
			token=EXCLUDED.token,
			timestamp=EXCLUDED.timestamp,
			sender_timestamp=COALESCE(EXCLUDED.sender_timestamp, whatsmeow_privacy_tokens.sender_timestamp)
		WHERE EXCLUDED.timestamp >= whatsmeow_privacy_tokens.timestamp
	`
	getPrivacyTokenQuery = `
		SELECT token, timestamp, sender_timestamp, their_jid FROM whatsmeow_privacy_tokens WHERE our_jid=$1 AND (their_jid=$2 OR their_jid=(
			CASE
				WHEN $2 LIKE '%@lid'
					THEN (SELECT pn || '@s.whatsapp.net' FROM whatsmeow_lid_map WHERE lid=replace($2, '@lid', ''))
				WHEN $2 LIKE '%@s.whatsapp.net'
					THEN (SELECT lid || '@lid' FROM whatsmeow_lid_map WHERE pn=replace($2, '@s.whatsapp.net', ''))
				ELSE $2
			END
		))
		ORDER BY timestamp DESC LIMIT 1
	`
)

func (s *sealedStore) PutPrivacyTokens(ctx context.Context, tokens ...wastore.PrivacyToken) error {
	args := make([]any, 1+len(tokens)*4)
	placeholders := make([]string, len(tokens))
	args[0] = s.ourJID
	for i, token := range tokens {
		user := token.User.ToNonAD().String()
		args[i*4+1] = user
		args[i*4+2] = s.sealNullable(tPrivacy, "token", token.Token, user)
		args[i*4+3] = token.Timestamp.Unix()
		if token.SenderTimestamp.IsZero() {
			args[i*4+4] = nil
		} else {
			args[i*4+4] = token.SenderTimestamp.Unix()
		}
		placeholders[i] = fmt.Sprintf("($1, $%d, $%d, $%d, $%d)", i*4+2, i*4+3, i*4+4, i*4+5)
	}
	query := strings.ReplaceAll(putPrivacyTokensQuery, "($1, $2, $3, $4, $5)", strings.Join(placeholders, ","))
	_, err := s.db.Exec(ctx, query, args...)
	return err
}

func (s *sealedStore) GetPrivacyToken(ctx context.Context, user types.JID) (*wastore.PrivacyToken, error) {
	token := wastore.PrivacyToken{User: user.ToNonAD()}
	var sealed []byte
	var ts int64
	var senderTS sql.NullInt64
	var stored string
	err := s.db.QueryRow(ctx, getPrivacyTokenQuery, s.ourJID, token.User).Scan(&sealed, &ts, &senderTS, &stored)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	if token.Token, err = s.open(tPrivacy, "token", sealed, stored); err != nil {
		return nil, err
	}
	token.Timestamp = time.Unix(ts, 0)
	if senderTS.Valid {
		token.SenderTimestamp = time.Unix(senderTS.Int64, 0)
	}
	return &token, nil
}

// The NCT salt: one row per account, its primary key is our JID.

func (s *sealedStore) PutNCTSalt(ctx context.Context, salt []byte) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO whatsmeow_nct_salt (our_jid, salt) VALUES ($1, $2)
		ON CONFLICT (our_jid) DO UPDATE SET salt=excluded.salt`,
		s.ourJID, s.sealNullable(tNCTSalt, "salt", salt))
	return err
}

func (s *sealedStore) GetNCTSalt(ctx context.Context) ([]byte, error) {
	var sealed []byte
	err := s.db.QueryRow(ctx, `SELECT salt FROM whatsmeow_nct_salt WHERE our_jid=$1`, s.ourJID).Scan(&sealed)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	return s.open(tNCTSalt, "salt", sealed)
}

// The event buffer: a NULL plaintext (none, or cleared) stays NULL.

func (s *sealedStore) GetBufferedEvent(ctx context.Context, ciphertextHash [32]byte) (*wastore.BufferedEvent, error) {
	var insertTimeMS, serverTimeSeconds int64
	var sealed []byte
	err := s.db.QueryRow(ctx, `
		SELECT plaintext, server_timestamp, insert_timestamp FROM whatsmeow_event_buffer WHERE our_jid = $1 AND ciphertext_hash = $2`,
		s.ourJID, ciphertextHash[:]).Scan(&sealed, &serverTimeSeconds, &insertTimeMS)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	plain, err := s.open(tEventBuffer, "plaintext", sealed, string(ciphertextHash[:]))
	if err != nil {
		return nil, err
	}
	return &wastore.BufferedEvent{
		Plaintext:  plain,
		ServerTime: time.Unix(serverTimeSeconds, 0),
		InsertTime: time.UnixMilli(insertTimeMS),
	}, nil
}

func (s *sealedStore) PutBufferedEvent(ctx context.Context, ciphertextHash [32]byte, plaintext []byte, serverTimestamp time.Time) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO whatsmeow_event_buffer (our_jid, ciphertext_hash, plaintext, server_timestamp, insert_timestamp)
		VALUES ($1, $2, $3, $4, $5)`,
		s.ourJID, ciphertextHash[:], s.sealNullable(tEventBuffer, "plaintext", plaintext, string(ciphertextHash[:])),
		serverTimestamp.Unix(), time.Now().UnixMilli())
	return err
}

// The retry buffer. The lookup is SQLStore's (the chat or its other address) and returns the
// stored chat for the AAD.

func (s *sealedStore) GetOutgoingEvent(ctx context.Context, chatJID, altChatJID types.JID, id types.MessageID) (string, []byte, error) {
	var format, storedChat string
	var sealed []byte
	err := s.db.QueryRow(ctx, `
		SELECT format, plaintext, chat_jid FROM whatsmeow_retry_buffer WHERE our_jid=$1 AND (chat_jid=$2 OR chat_jid=$3) AND message_id=$4`,
		s.ourJID, chatJID, altChatJID, id).Scan(&format, &sealed, &storedChat)
	if err != nil {
		// Like SQLStore, sql.ErrNoRows reaches the caller.
		return "", nil, err
	}
	plain, err := s.open(tRetryBuffer, "plaintext", sealed, storedChat, id)
	if err != nil {
		return "", nil, err
	}
	return format, plain, nil
}

func (s *sealedStore) AddOutgoingEvent(ctx context.Context, chatJID types.JID, id types.MessageID, format string, plaintext []byte) error {
	_, err := s.db.Exec(ctx, `
		INSERT INTO whatsmeow_retry_buffer (our_jid, chat_jid, message_id, format, plaintext, timestamp)
		VALUES ($1, $2, $3, $4, $5, $6)
		ON CONFLICT (our_jid, chat_jid, message_id) DO UPDATE
			SET format=excluded.format, plaintext=excluded.plaintext, timestamp=excluded.timestamp`,
		s.ourJID, chatJID, id, format, s.sealNullable(tRetryBuffer, "plaintext", plaintext, chatJID.String(), id), time.Now().UnixMilli())
	return err
}
