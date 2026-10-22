package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/util/keys"
)

// Fake addresses: every number starts with 1555 or is an obviously synthetic LID.
const (
	addrA     = "15551230002:0"
	addrB     = "15551230003:0"
	addrC     = "15551230004:1"
	testGroup = "120363000000000001@g.us"
)

var (
	peerPN       = types.JID{User: "15551230002", Server: types.DefaultUserServer}
	peerLID      = types.JID{User: "100000000000002", Server: types.HiddenUserServer}
	otherPeer    = types.JID{User: "15551230003", Server: types.DefaultUserServer}
	testGroupJID = types.JID{User: "120363000000000001", Server: types.GroupServer}
)

// sealedDevice saves a paired device through a DeviceStore over a fresh nuncio.db in dir and
// returns it with its sealed store.
func sealedDevice(t *testing.T, dir string) (*wastore.Device, *sealedStore, *sql.DB) {
	t.Helper()
	db := openStoreDB(t, dir)
	device := pairedDevice(t, ourTestJID)
	device.Container = newTestDeviceStore(t, db, testSealer(t))
	if err := device.Save(context.Background()); err != nil {
		t.Fatalf("Save: %v", err)
	}
	st, ok := device.Sessions.(*sealedStore)
	if !ok {
		t.Fatalf("the device's stores are %T, want *sealedStore", device.Sessions)
	}
	return device, st, db
}

// stockDevice saves a copy of like through whatsmeow's own container over a fresh nuncio.db in
// dir, so its stores are the stock *sqlstore.SQLStore.
func stockDevice(t *testing.T, dir string, like *wastore.Device) (*wastore.Device, *sql.DB) {
	t.Helper()
	ctx := context.Background()
	db := openStoreDB(t, dir)
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	if err := container.Upgrade(ctx); err != nil {
		t.Fatalf("Upgrade: %v", err)
	}
	device := *like
	device.Initialized = false
	device.Container = container
	if err := container.PutDevice(ctx, &device); err != nil {
		t.Fatalf("stock PutDevice: %v", err)
	}
	if _, ok := device.Sessions.(*sqlstore.SQLStore); !ok {
		t.Fatalf("stock stores are %T", device.Sessions)
	}
	return &device, db
}

// testSecrets are the private values a workload stores, one per sealed column.
type testSecrets struct {
	sessionA, sessionB, sessionC, sessionD []byte
	senderKey                              []byte
	appKey1, appKey2, appKeyStale          []byte
	msgSecret, msgSecret2                  []byte
	token, token2                          []byte
	salt                                   []byte
	buffered, retry                        []byte
}

func newTestSecrets(t *testing.T) testSecrets {
	return testSecrets{
		sessionA: randBytes(t, 48), sessionB: randBytes(t, 48), sessionC: randBytes(t, 48), sessionD: randBytes(t, 48),
		senderKey: randBytes(t, 48),
		appKey1:   randBytes(t, 32), appKey2: randBytes(t, 32), appKeyStale: randBytes(t, 32),
		msgSecret: randBytes(t, 32), msgSecret2: randBytes(t, 32),
		token: randBytes(t, 24), token2: randBytes(t, 24),
		salt:     randBytes(t, 32),
		buffered: randBytes(t, 40), retry: randBytes(t, 40),
	}
}

func (s testSecrets) named() map[string][]byte {
	return map[string][]byte{
		"session A": s.sessionA, "session B": s.sessionB, "session C": s.sessionC, "session D": s.sessionD,
		"sender key":      s.senderKey,
		"app-state key 1": s.appKey1, "app-state key 2": s.appKey2, "stale app-state key": s.appKeyStale,
		"message secret": s.msgSecret, "message secret 2": s.msgSecret2,
		"privacy token": s.token, "privacy token 2": s.token2,
		"NCT salt": s.salt, "buffered event": s.buffered, "retry plaintext": s.retry,
	}
}

type observed struct {
	label string
	value any
}

// exercise drives every overridden method through the device's stores and returns what the
// calls observed. It leaves out what is random per store (pre-key bytes, insert times) and checks
// those for consistency within the store instead. preKeys collects the generated pre-keys'
// private bytes.
func exercise(t *testing.T, d *wastore.Device, sec testSecrets, preKeys map[string][]byte) []observed {
	t.Helper()
	ctx := context.Background()
	var out []observed
	rec := func(label string, value any, err error) {
		t.Helper()
		if err != nil {
			t.Fatalf("%s: %v", label, err)
		}
		out = append(out, observed{label, value})
	}

	// Sessions.
	rec("PutSession A", nil, d.Sessions.PutSession(ctx, addrA, sec.sessionA))
	rec("PutSession B", nil, d.Sessions.PutSession(ctx, addrB, sec.sessionB))
	v, err := d.Sessions.GetSession(ctx, addrA)
	rec("GetSession A", v, err)
	v, err = d.Sessions.GetSession(ctx, "15551239999:0")
	rec("GetSession missing", v, err)
	m, err := d.Sessions.GetManySessions(ctx, []string{addrA, addrB, "15551239999:0"})
	rec("GetManySessions", m, err)
	m, err = d.Sessions.GetManySessions(ctx, nil)
	rec("GetManySessions none", m, err)
	rec("PutManySessions", nil, d.Sessions.PutManySessions(ctx, map[string][]byte{addrC: sec.sessionC, "15551230005:0": sec.sessionD}))
	v, err = d.Sessions.GetSession(ctx, addrC)
	rec("GetSession C", v, err)
	has, err := d.Sessions.HasSession(ctx, addrC)
	rec("HasSession C", has, err)
	rec("PutSession A again", nil, d.Sessions.PutSession(ctx, addrA, sec.sessionD))
	v, err = d.Sessions.GetSession(ctx, addrA)
	rec("GetSession A replaced", v, err)
	rec("DeleteSession B", nil, d.Sessions.DeleteSession(ctx, addrB))
	v, err = d.Sessions.GetSession(ctx, addrB)
	rec("GetSession B deleted", v, err)

	// Pre-keys.
	ids := func(ks []*keys.PreKey) []uint32 {
		var out []uint32
		for _, k := range ks {
			out = append(out, k.KeyID)
		}
		return out
	}
	first, err := d.PreKeys.GetOrGenPreKeys(ctx, 5)
	rec("GetOrGenPreKeys 5", ids(first), err)
	for _, k := range first {
		preKeys[fmt.Sprintf("pre-key %d", k.KeyID)] = k.Priv[:]
	}
	same, err := d.PreKeys.GetOrGenPreKeys(ctx, 3)
	rec("GetOrGenPreKeys 3 again", ids(same), err)
	for i, k := range same {
		if *k.Priv != *first[i].Priv || *k.Pub != *first[i].Pub {
			t.Errorf("GetOrGenPreKeys did not return the stored pre-key %d", k.KeyID)
		}
	}
	got, err := d.PreKeys.GetPreKey(ctx, 2)
	rec("GetPreKey 2 id", got.KeyID, err)
	if *got.Priv != *first[1].Priv || *got.Pub != *first[1].Pub {
		t.Errorf("GetPreKey(2) differs from the generated key")
	}
	n, err := d.PreKeys.UploadedPreKeyCount(ctx)
	rec("UploadedPreKeyCount before", n, err)
	rec("MarkPreKeysAsUploaded 3", nil, d.PreKeys.MarkPreKeysAsUploaded(ctx, 3))
	n, err = d.PreKeys.UploadedPreKeyCount(ctx)
	rec("UploadedPreKeyCount after", n, err)
	more, err := d.PreKeys.GetOrGenPreKeys(ctx, 4)
	rec("GetOrGenPreKeys 4 after upload", ids(more), err)
	for _, k := range more {
		preKeys[fmt.Sprintf("pre-key %d", k.KeyID)] = k.Priv[:]
	}
	one, err := d.PreKeys.GenOnePreKey(ctx)
	rec("GenOnePreKey id", one.KeyID, err)
	preKeys[fmt.Sprintf("pre-key %d", one.KeyID)] = one.Priv[:]
	n, err = d.PreKeys.UploadedPreKeyCount(ctx)
	rec("UploadedPreKeyCount after GenOne", n, err)
	rec("RemovePreKey 2", nil, d.PreKeys.RemovePreKey(ctx, 2))
	got, err = d.PreKeys.GetPreKey(ctx, 2)
	rec("GetPreKey removed", got == nil, err)
	got, err = d.PreKeys.GetPreKey(ctx, 999)
	rec("GetPreKey missing", got == nil, err)

	// Sender keys.
	rec("PutSenderKey", nil, d.SenderKeys.PutSenderKey(ctx, testGroup, addrA, sec.senderKey))
	v, err = d.SenderKeys.GetSenderKey(ctx, testGroup, addrA)
	rec("GetSenderKey", v, err)
	v, err = d.SenderKeys.GetSenderKey(ctx, testGroup, addrB)
	rec("GetSenderKey missing", v, err)

	// App-state sync keys.
	id1, id2 := []byte("app-key-id-1"), []byte("app-key-id-2")
	rec("PutAppStateSyncKey 1", nil, d.AppStateKeys.PutAppStateSyncKey(ctx, id1, wastore.AppStateSyncKey{Data: sec.appKey1, Fingerprint: []byte("fp1"), Timestamp: 10}))
	rec("PutAppStateSyncKey 2", nil, d.AppStateKeys.PutAppStateSyncKey(ctx, id2, wastore.AppStateSyncKey{Data: sec.appKey2, Fingerprint: []byte("fp2"), Timestamp: 20}))
	rec("PutAppStateSyncKey 1 stale", nil, d.AppStateKeys.PutAppStateSyncKey(ctx, id1, wastore.AppStateSyncKey{Data: sec.appKeyStale, Fingerprint: []byte("fpx"), Timestamp: 5}))
	ak, err := d.AppStateKeys.GetAppStateSyncKey(ctx, id1)
	rec("GetAppStateSyncKey 1", ak, err)
	ak, err = d.AppStateKeys.GetAppStateSyncKey(ctx, []byte("missing"))
	rec("GetAppStateSyncKey missing", ak, err)
	all, err := d.AppStateKeys.GetAllAppStateSyncKeys(ctx)
	rec("GetAllAppStateSyncKeys", all, err)
	latest, err := d.AppStateKeys.GetLatestAppStateSyncKeyID(ctx)
	rec("GetLatestAppStateSyncKeyID", latest, err)

	// Message secrets, also found through the LID map.
	rec("PutLIDMapping", nil, d.LIDs.PutLIDMapping(ctx, peerLID, peerPN))
	rec("PutMessageSecret", nil, d.MsgSecrets.PutMessageSecret(ctx, peerPN, peerPN, "MSGID1", sec.msgSecret))
	rec("PutMessageSecrets", nil, d.MsgSecrets.PutMessageSecrets(ctx, []wastore.MessageSecretInsert{
		{Chat: testGroupJID, Sender: otherPeer, ID: "MSGID2", Secret: sec.msgSecret2},
		{Chat: testGroupJID, Sender: otherPeer, ID: "", Secret: sec.msgSecret}, // skipped: no id
	}))
	secret, sender, err := d.MsgSecrets.GetMessageSecret(ctx, peerPN, peerPN, "MSGID1")
	rec("GetMessageSecret", []any{secret, sender}, err)
	secret, sender, err = d.MsgSecrets.GetMessageSecret(ctx, peerLID, peerLID, "MSGID1")
	rec("GetMessageSecret through the LID", []any{secret, sender}, err)
	secret, sender, err = d.MsgSecrets.GetMessageSecret(ctx, testGroupJID, otherPeer, "MSGID2")
	rec("GetMessageSecret 2", []any{secret, sender}, err)
	secret, sender, err = d.MsgSecrets.GetMessageSecret(ctx, testGroupJID, otherPeer, "")
	rec("GetMessageSecret skipped", []any{secret, sender}, err)

	// Privacy tokens.
	rec("PutPrivacyTokens", nil, d.PrivacyTokens.PutPrivacyTokens(ctx,
		wastore.PrivacyToken{User: peerPN, Token: sec.token, Timestamp: time.Unix(1000, 0)},
		wastore.PrivacyToken{User: otherPeer, Token: sec.token2, Timestamp: time.Unix(2000, 0), SenderTimestamp: time.Unix(1990, 0)},
	))
	rec("PutPrivacyTokens older", nil, d.PrivacyTokens.PutPrivacyTokens(ctx,
		wastore.PrivacyToken{User: peerPN, Token: sec.token2, Timestamp: time.Unix(500, 0)}))
	pt, err := d.PrivacyTokens.GetPrivacyToken(ctx, peerPN)
	rec("GetPrivacyToken", pt, err)
	pt, err = d.PrivacyTokens.GetPrivacyToken(ctx, peerLID)
	rec("GetPrivacyToken through the LID", pt, err)
	pt, err = d.PrivacyTokens.GetPrivacyToken(ctx, otherPeer)
	rec("GetPrivacyToken 2", pt, err)
	pt, err = d.PrivacyTokens.GetPrivacyToken(ctx, types.JID{User: "15551239999", Server: types.DefaultUserServer})
	rec("GetPrivacyToken missing", pt, err)

	// NCT salt.
	v, err = d.NCTSalt.GetNCTSalt(ctx)
	rec("GetNCTSalt empty", v, err)
	rec("PutNCTSalt", nil, d.NCTSalt.PutNCTSalt(ctx, sec.salt))
	v, err = d.NCTSalt.GetNCTSalt(ctx)
	rec("GetNCTSalt", v, err)

	// The event buffer.
	hash1, hash2 := [32]byte{1}, [32]byte{2}
	rec("PutBufferedEvent", nil, d.EventBuffer.PutBufferedEvent(ctx, hash1, sec.buffered, time.Unix(3000, 0)))
	rec("PutBufferedEvent no plaintext", nil, d.EventBuffer.PutBufferedEvent(ctx, hash2, nil, time.Unix(3001, 0)))
	buffered := func(label string, hash [32]byte) {
		ev, err := d.EventBuffer.GetBufferedEvent(ctx, hash)
		if ev == nil {
			rec(label, nil, err)
			return
		}
		rec(label, []any{ev.Plaintext, ev.ServerTime}, err)
	}
	buffered("GetBufferedEvent", hash1)
	buffered("GetBufferedEvent no plaintext", hash2)
	buffered("GetBufferedEvent missing", [32]byte{3})
	rec("ClearBufferedEventPlaintext", nil, d.EventBuffer.ClearBufferedEventPlaintext(ctx, hash1))
	buffered("GetBufferedEvent cleared", hash1)

	// The retry buffer.
	rec("AddOutgoingEvent", nil, d.EventBuffer.AddOutgoingEvent(ctx, peerLID, "MSGID3", "proto", sec.retry))
	format, plain, err := d.EventBuffer.GetOutgoingEvent(ctx, peerLID, peerPN, "MSGID3")
	rec("GetOutgoingEvent", []any{format, plain}, err)
	format, plain, err = d.EventBuffer.GetOutgoingEvent(ctx, peerPN, peerLID, "MSGID3")
	rec("GetOutgoingEvent by the alt JID", []any{format, plain}, err)
	_, _, err = d.EventBuffer.GetOutgoingEvent(ctx, peerPN, peerLID, "MSGID4")
	rec("GetOutgoingEvent missing is ErrNoRows", errors.Is(err, sql.ErrNoRows), nil)

	// The decryption transaction carries sealed reads and writes.
	err = d.EventBuffer.DoDecryptionTxn(ctx, func(ctx context.Context) error {
		if err := d.Sessions.PutSession(ctx, addrB, sec.sessionB); err != nil {
			return err
		}
		got, err := d.Sessions.GetSession(ctx, addrB)
		out = append(out, observed{"GetSession inside the txn", got})
		return err
	})
	rec("DoDecryptionTxn", nil, err)
	return out
}

func TestSealedStoreBehavesLikeTheStockStore(t *testing.T) {
	sec := newTestSecrets(t)
	device, _, _ := sealedDevice(t, t.TempDir())
	stock, _ := stockDevice(t, t.TempDir(), device)

	got := exercise(t, device, sec, map[string][]byte{})
	want := exercise(t, stock, sec, map[string][]byte{})
	if len(got) != len(want) {
		t.Fatalf("observed %d results, stock %d", len(got), len(want))
	}
	for i := range got {
		if got[i].label != want[i].label || !reflect.DeepEqual(got[i].value, want[i].value) {
			t.Errorf("%s: sealed store %#v, stock store %#v", want[i].label, got[i].value, want[i].value)
		}
	}
}

// After a realistic workload, no private byte string is in nuncio.db, its -wal or -shm, raw or
// in base64, while the database is open and after it is closed. The same workload on the stock
// store leaves every one of them in its files, so the scan can fail.
func TestSealedStoreKeepsEverySecretOutOfTheDBFiles(t *testing.T) {
	sec := newTestSecrets(t)
	dir := t.TempDir()
	device, _, db := sealedDevice(t, dir)
	preKeys := map[string][]byte{}
	exercise(t, device, sec, preKeys)

	secrets := sec.named()
	for name, value := range devicePrivate(device) {
		secrets[name] = value
	}
	for name, value := range preKeys {
		secrets[name] = value
	}
	if _, ok := dbFiles(t, dir)[DBFile+"-wal"]; !ok {
		t.Fatal("no -wal file while the database is open: the scan would miss it")
	}
	assertNoSecretsOnDisk(t, dir, secrets)
	if err := db.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	assertNoSecretsOnDisk(t, dir, secrets)

	stockDir := t.TempDir()
	stock, stockDB := stockDevice(t, stockDir, device)
	stockPreKeys := map[string][]byte{}
	exercise(t, stock, sec, stockPreKeys)
	if err := stockDB.Close(); err != nil {
		t.Fatalf("close the stock database: %v", err)
	}
	stockSecrets := sec.named()
	delete(stockSecrets, "stale app-state key") // never stored: its timestamp is older
	delete(stockSecrets, "buffered event")      // cleared by the workload
	delete(stockSecrets, "session A")           // overwritten by the workload
	for name, value := range devicePrivate(device) {
		stockSecrets[name] = value
	}
	for name, value := range stockPreKeys {
		if name != "pre-key 2" { // removed by the workload
			stockSecrets[name] = value
		}
	}
	files := dbFiles(t, stockDir)
	for name, value := range stockSecrets {
		found := false
		for _, data := range files {
			found = found || bytesContains(data, value)
		}
		if !found {
			t.Errorf("control: %s is not in the stock store's files, so the scan proves nothing for it", name)
		}
	}
}

func bytesContains(data, value []byte) bool { return strings.Contains(string(data), string(value)) }

// A sealed value moved to another row, column, table or account fails to open, with an error
// that names the table and nothing else.
func TestSealedValuesAreBoundToTheirRow(t *testing.T) {
	ctx := context.Background()
	otherJID := types.JID{User: "15551230000", Server: types.DefaultUserServer, Device: 3}
	sec := newTestSecrets(t)
	cases := []struct {
		name  string
		table string
		setup func(t *testing.T, d *wastore.Device)
		move  string
		read  func(d, other *wastore.Device) error
	}{
		{
			name: "session to another address", table: "whatsmeow_sessions",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.Sessions.PutSession(ctx, addrA, sec.sessionA))
				must(t, d.Sessions.PutSession(ctx, addrB, sec.sessionB))
			},
			move: `UPDATE whatsmeow_sessions SET session = (SELECT session FROM whatsmeow_sessions WHERE their_id = '` + addrB + `') WHERE their_id = '` + addrA + `'`,
			read: func(d, _ *wastore.Device) error { _, err := d.Sessions.GetSession(ctx, addrA); return err },
		},
		{
			name: "session read through GetManySessions", table: "whatsmeow_sessions",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.Sessions.PutSession(ctx, addrA, sec.sessionA))
				must(t, d.Sessions.PutSession(ctx, addrB, sec.sessionB))
			},
			move: `UPDATE whatsmeow_sessions SET session = (SELECT session FROM whatsmeow_sessions WHERE their_id = '` + addrB + `') WHERE their_id = '` + addrA + `'`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.Sessions.GetManySessions(ctx, []string{addrA})
				return err
			},
		},
		{
			name: "sender key to another sender", table: "whatsmeow_sender_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.SenderKeys.PutSenderKey(ctx, testGroup, addrA, sec.sessionA))
				must(t, d.SenderKeys.PutSenderKey(ctx, testGroup, addrB, sec.sessionB))
			},
			move: `UPDATE whatsmeow_sender_keys SET sender_key = (SELECT sender_key FROM whatsmeow_sender_keys WHERE sender_id = '` + addrB + `') WHERE sender_id = '` + addrA + `'`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.SenderKeys.GetSenderKey(ctx, testGroup, addrA)
				return err
			},
		},
		{
			name: "sender key to another chat", table: "whatsmeow_sender_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.SenderKeys.PutSenderKey(ctx, testGroup, addrA, sec.sessionA))
				must(t, d.SenderKeys.PutSenderKey(ctx, "120363000000000009@g.us", addrA, sec.sessionB))
			},
			move: `UPDATE whatsmeow_sender_keys SET sender_key = (SELECT sender_key FROM whatsmeow_sender_keys WHERE chat_id = '120363000000000009@g.us') WHERE chat_id = '` + testGroup + `'`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.SenderKeys.GetSenderKey(ctx, testGroup, addrA)
				return err
			},
		},
		{
			name: "session moved into the sender keys", table: "whatsmeow_sender_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.Sessions.PutSession(ctx, addrA, sec.sessionA))
				must(t, d.SenderKeys.PutSenderKey(ctx, testGroup, addrA, sec.sessionB))
			},
			move: `UPDATE whatsmeow_sender_keys SET sender_key = (SELECT session FROM whatsmeow_sessions WHERE their_id = '` + addrA + `')`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.SenderKeys.GetSenderKey(ctx, testGroup, addrA)
				return err
			},
		},
		{
			name: "app-state key to another key id", table: "whatsmeow_app_state_sync_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.AppStateKeys.PutAppStateSyncKey(ctx, []byte("k1"), wastore.AppStateSyncKey{Data: sec.appKey1, Fingerprint: []byte("f"), Timestamp: 1}))
				must(t, d.AppStateKeys.PutAppStateSyncKey(ctx, []byte("k2"), wastore.AppStateSyncKey{Data: sec.appKey2, Fingerprint: []byte("f"), Timestamp: 2}))
			},
			move: `UPDATE whatsmeow_app_state_sync_keys SET key_data = (SELECT key_data FROM whatsmeow_app_state_sync_keys WHERE key_id = CAST('k2' AS BLOB)) WHERE key_id = CAST('k1' AS BLOB)`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.AppStateKeys.GetAppStateSyncKey(ctx, []byte("k1"))
				return err
			},
		},
		{
			name: "app-state key read through GetAll", table: "whatsmeow_app_state_sync_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.AppStateKeys.PutAppStateSyncKey(ctx, []byte("k1"), wastore.AppStateSyncKey{Data: sec.appKey1, Fingerprint: []byte("f"), Timestamp: 1}))
				must(t, d.AppStateKeys.PutAppStateSyncKey(ctx, []byte("k2"), wastore.AppStateSyncKey{Data: sec.appKey2, Fingerprint: []byte("f"), Timestamp: 2}))
			},
			move: `UPDATE whatsmeow_app_state_sync_keys SET key_data = (SELECT key_data FROM whatsmeow_app_state_sync_keys WHERE key_id = CAST('k2' AS BLOB)) WHERE key_id = CAST('k1' AS BLOB)`,
			read: func(d, _ *wastore.Device) error { _, err := d.AppStateKeys.GetAllAppStateSyncKeys(ctx); return err },
		},
		{
			name: "message secret to another message", table: "whatsmeow_message_secrets",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.MsgSecrets.PutMessageSecret(ctx, testGroupJID, peerPN, "M1", sec.msgSecret))
				must(t, d.MsgSecrets.PutMessageSecret(ctx, testGroupJID, peerPN, "M2", sec.msgSecret2))
			},
			move: `UPDATE whatsmeow_message_secrets SET key = (SELECT key FROM whatsmeow_message_secrets WHERE message_id = 'M2') WHERE message_id = 'M1'`,
			read: func(d, _ *wastore.Device) error {
				_, _, err := d.MsgSecrets.GetMessageSecret(ctx, testGroupJID, peerPN, "M1")
				return err
			},
		},
		{
			name: "message secret to another sender", table: "whatsmeow_message_secrets",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.MsgSecrets.PutMessageSecret(ctx, testGroupJID, peerPN, "M1", sec.msgSecret))
				must(t, d.MsgSecrets.PutMessageSecret(ctx, testGroupJID, otherPeer, "M1", sec.msgSecret2))
			},
			move: `UPDATE whatsmeow_message_secrets SET key = (SELECT key FROM whatsmeow_message_secrets WHERE sender_jid = '` + otherPeer.String() + `') WHERE sender_jid = '` + peerPN.String() + `'`,
			read: func(d, _ *wastore.Device) error {
				_, _, err := d.MsgSecrets.GetMessageSecret(ctx, testGroupJID, peerPN, "M1")
				return err
			},
		},
		{
			name: "message secret to another chat", table: "whatsmeow_message_secrets",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.MsgSecrets.PutMessageSecret(ctx, testGroupJID, peerPN, "M1", sec.msgSecret))
				must(t, d.MsgSecrets.PutMessageSecret(ctx, peerPN, peerPN, "M1", sec.msgSecret2))
			},
			move: `UPDATE whatsmeow_message_secrets SET key = (SELECT key FROM whatsmeow_message_secrets WHERE chat_jid = '` + peerPN.String() + `') WHERE chat_jid = '` + testGroupJID.String() + `'`,
			read: func(d, _ *wastore.Device) error {
				_, _, err := d.MsgSecrets.GetMessageSecret(ctx, testGroupJID, peerPN, "M1")
				return err
			},
		},
		{
			name: "privacy token to another contact", table: "whatsmeow_privacy_tokens",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.PrivacyTokens.PutPrivacyTokens(ctx,
					wastore.PrivacyToken{User: peerPN, Token: sec.token, Timestamp: time.Unix(1, 0)},
					wastore.PrivacyToken{User: otherPeer, Token: sec.token2, Timestamp: time.Unix(1, 0)}))
			},
			move: `UPDATE whatsmeow_privacy_tokens SET token = (SELECT token FROM whatsmeow_privacy_tokens WHERE their_jid = '` + otherPeer.String() + `') WHERE their_jid = '` + peerPN.String() + `'`,
			read: func(d, _ *wastore.Device) error { _, err := d.PrivacyTokens.GetPrivacyToken(ctx, peerPN); return err },
		},
		{
			name: "NCT salt to another account", table: "whatsmeow_nct_salt",
			setup: func(t *testing.T, d *wastore.Device) { must(t, d.NCTSalt.PutNCTSalt(ctx, sec.salt)) },
			move:  `INSERT INTO whatsmeow_nct_salt (our_jid, salt) SELECT '` + otherJID.String() + `', salt FROM whatsmeow_nct_salt`,
			read:  func(_, other *wastore.Device) error { _, err := other.NCTSalt.GetNCTSalt(ctx); return err },
		},
		{
			name: "buffered event to another hash", table: "whatsmeow_event_buffer",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.EventBuffer.PutBufferedEvent(ctx, [32]byte{1}, sec.buffered, time.Unix(1, 0)))
				must(t, d.EventBuffer.PutBufferedEvent(ctx, [32]byte{2}, sec.retry, time.Unix(1, 0)))
			},
			move: `UPDATE whatsmeow_event_buffer SET plaintext = (SELECT plaintext FROM whatsmeow_event_buffer WHERE ciphertext_hash = ?) WHERE ciphertext_hash = ?`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.EventBuffer.GetBufferedEvent(ctx, [32]byte{1})
				return err
			},
		},
		{
			name: "retry plaintext to another message", table: "whatsmeow_retry_buffer",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.EventBuffer.AddOutgoingEvent(ctx, peerPN, "M1", "proto", sec.retry))
				must(t, d.EventBuffer.AddOutgoingEvent(ctx, peerPN, "M2", "proto", sec.buffered))
			},
			move: `UPDATE whatsmeow_retry_buffer SET plaintext = (SELECT plaintext FROM whatsmeow_retry_buffer WHERE message_id = 'M2') WHERE message_id = 'M1'`,
			read: func(d, _ *wastore.Device) error {
				_, _, err := d.EventBuffer.GetOutgoingEvent(ctx, peerPN, peerLID, "M1")
				return err
			},
		},
		{
			name: "retry plaintext to another chat", table: "whatsmeow_retry_buffer",
			setup: func(t *testing.T, d *wastore.Device) {
				must(t, d.EventBuffer.AddOutgoingEvent(ctx, peerPN, "M1", "proto", sec.retry))
				must(t, d.EventBuffer.AddOutgoingEvent(ctx, otherPeer, "M1", "proto", sec.buffered))
			},
			move: `UPDATE whatsmeow_retry_buffer SET plaintext = (SELECT plaintext FROM whatsmeow_retry_buffer WHERE chat_jid = '` + otherPeer.String() + `') WHERE chat_jid = '` + peerPN.String() + `'`,
			read: func(d, _ *wastore.Device) error {
				_, _, err := d.EventBuffer.GetOutgoingEvent(ctx, peerPN, peerLID, "M1")
				return err
			},
		},
		{
			name: "pre-key to another key id", table: "nuncio_pre_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				_, err := d.PreKeys.GetOrGenPreKeys(ctx, 2)
				must(t, err)
			},
			move: `UPDATE nuncio_pre_keys SET sealed = (SELECT sealed FROM nuncio_pre_keys WHERE key_id = 2) WHERE key_id = 1`,
			read: func(d, _ *wastore.Device) error { _, err := d.PreKeys.GetPreKey(ctx, 1); return err },
		},
		{
			name: "pre-key read through GetOrGenPreKeys", table: "nuncio_pre_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				_, err := d.PreKeys.GetOrGenPreKeys(ctx, 2)
				must(t, err)
			},
			move: `UPDATE nuncio_pre_keys SET sealed = (SELECT sealed FROM nuncio_pre_keys WHERE key_id = 2) WHERE key_id = 1`,
			read: func(d, _ *wastore.Device) error { _, err := d.PreKeys.GetOrGenPreKeys(ctx, 2); return err },
		},
		{
			name: "pre-key to another account", table: "nuncio_pre_keys",
			setup: func(t *testing.T, d *wastore.Device) {
				_, err := d.PreKeys.GetOrGenPreKeys(ctx, 1)
				must(t, err)
			},
			move: `INSERT INTO nuncio_pre_keys (jid, key_id, sealed, uploaded) SELECT '` + otherJID.String() + `', key_id, sealed, uploaded FROM nuncio_pre_keys`,
			read: func(_, other *wastore.Device) error { _, err := other.PreKeys.GetPreKey(ctx, 1); return err },
		},
		{
			name: "device keys to another account", table: "nuncio_device",
			setup: func(t *testing.T, d *wastore.Device) {},
			move:  `UPDATE nuncio_device SET sealed = (SELECT sealed FROM nuncio_device WHERE jid = '` + ourTestJID.String() + `') WHERE jid = '` + otherJID.String() + `'`,
			read: func(d, _ *wastore.Device) error {
				_, err := d.Container.(*DeviceStore).LoadDevice(ctx) // loads otherJID, the lowest JID
				return err
			},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			device, _, db := sealedDevice(t, t.TempDir())
			other := pairedDevice(t, otherJID)
			other.Container = device.Container
			must(t, other.Save(ctx))
			tc.setup(t, device)
			if tc.read(device, other) != nil {
				t.Fatalf("the value does not open before the move")
			}
			args := []any{}
			if strings.Contains(tc.move, "?") {
				h1, h2 := [32]byte{1}, [32]byte{2}
				args = append(args, h2[:], h1[:])
			}
			if _, err := db.Exec(tc.move, args...); err != nil {
				t.Fatalf("move: %v", err)
			}
			err := tc.read(device, other)
			if !errors.Is(err, ErrUnseal) {
				t.Fatalf("read after the move = %v, want ErrUnseal", err)
			}
			msg := err.Error()
			if !strings.Contains(msg, tc.table) {
				t.Errorf("error %q does not name the table %s", msg, tc.table)
			}
			for _, leak := range []string{"1555", "1000000", "M1", "M2", "k1", "k2", "@"} {
				if strings.Contains(msg, leak) {
					t.Errorf("error %q carries %q", msg, leak)
				}
			}
		})
	}
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

// Deleting the device removes every row of it, the sealed ones included, through the foreign keys.
func TestDeletingTheDeviceCascades(t *testing.T) {
	ctx := context.Background()
	device, _, db := sealedDevice(t, t.TempDir())
	exercise(t, device, newTestSecrets(t), map[string][]byte{})
	must(t, device.Identities.PutIdentity(ctx, addrA, [32]byte{9}))
	if _, _, err := device.Contacts.PutPushName(ctx, peerPN, "synthetic"); err != nil {
		t.Fatal(err)
	}

	tables := map[string]string{
		"whatsmeow_device": "jid", "nuncio_device": "jid", "nuncio_pre_keys": "jid",
		"whatsmeow_sessions": "our_jid", "whatsmeow_sender_keys": "our_jid", "whatsmeow_identity_keys": "our_jid",
		"whatsmeow_app_state_sync_keys": "jid", "whatsmeow_message_secrets": "our_jid", "whatsmeow_privacy_tokens": "our_jid",
		"whatsmeow_nct_salt": "our_jid", "whatsmeow_event_buffer": "our_jid", "whatsmeow_retry_buffer": "our_jid",
	}
	count := func(table, col string) int {
		var n int
		must(t, db.QueryRow(`SELECT COUNT(*) FROM `+table+` WHERE `+col+` = ?`, ourTestJID.String()).Scan(&n))
		return n
	}
	for table, col := range tables {
		if count(table, col) == 0 {
			t.Fatalf("the workload left no row in %s to delete", table)
		}
	}
	must(t, device.Delete(ctx))
	for table, col := range tables {
		if n := count(table, col); n != 0 {
			t.Errorf("%s still holds %d rows of the deleted device", table, n)
		}
	}
	if _, err := device.Container.(*DeviceStore).LoadDevice(ctx); !errors.Is(err, ErrNoDevice) {
		t.Errorf("LoadDevice after Delete = %v, want ErrNoDevice", err)
	}
}

// The PN-to-LID migration moves the same rows as whatsmeow's, sealed again under the LID address,
// and a second call for the same PN does nothing.
func TestMigratePNToLIDMovesAndReseals(t *testing.T) {
	ctx := context.Background()
	sec := newTestSecrets(t)
	pnAddrs := []string{"15551230002:0", "15551230002:3"}
	// whatsmeow addresses a LID user with its agent: "<lid>_1".
	lidUser := peerLID.SignalAddressUser()
	lidAddrs := []string{lidUser + ":0", lidUser + ":3"}
	untouched := []string{"155512300021:0", "15551230003:0"}
	identity, otherIdentity := [32]byte{7}, [32]byte{8}

	run := func(d *wastore.Device) []observed {
		var out []observed
		rec := func(label string, value any, err error) {
			t.Helper()
			must(t, err)
			out = append(out, observed{label, value})
		}
		must(t, d.Sessions.PutSession(ctx, pnAddrs[0], sec.sessionA))
		must(t, d.Sessions.PutSession(ctx, pnAddrs[1], sec.sessionB))
		must(t, d.Sessions.PutSession(ctx, untouched[0], sec.sessionC))
		must(t, d.Sessions.PutSession(ctx, untouched[1], sec.sessionD))
		must(t, d.Sessions.PutSession(ctx, lidAddrs[1], sec.salt)) // overwritten by the migration
		must(t, d.SenderKeys.PutSenderKey(ctx, testGroup, pnAddrs[0], sec.senderKey))
		must(t, d.Identities.PutIdentity(ctx, pnAddrs[0], identity))

		must(t, d.Sessions.MigratePNToLID(ctx, peerPN, peerLID))
		for _, addr := range slices.Concat(pnAddrs, lidAddrs, untouched) {
			v, err := d.Sessions.GetSession(ctx, addr)
			rec("session "+addr, v, err)
		}
		for _, addr := range []string{pnAddrs[0], lidAddrs[0]} {
			v, err := d.SenderKeys.GetSenderKey(ctx, testGroup, addr)
			rec("sender key "+addr, v, err)
			trusted, err := d.Identities.IsTrustedIdentity(ctx, addr, otherIdentity)
			rec("identity unknown or other "+addr, trusted, err)
		}

		// A second call for the same PN is a no-op, even with a new PN row.
		must(t, d.Sessions.PutSession(ctx, "15551230002:5", sec.sessionC))
		must(t, d.Sessions.MigratePNToLID(ctx, peerPN, peerLID))
		for _, addr := range []string{"15551230002:5", lidUser + ":5"} {
			v, err := d.Sessions.GetSession(ctx, addr)
			rec("after the second call "+addr, v, err)
		}
		return out
	}

	device, _, db := sealedDevice(t, t.TempDir())
	stock, _ := stockDevice(t, t.TempDir(), device)
	got, want := run(device), run(stock)
	for i := range want {
		if got[i].label != want[i].label || !reflect.DeepEqual(got[i].value, want[i].value) {
			t.Errorf("%s: sealed store %x, stock store %x", want[i].label, got[i].value, want[i].value)
		}
	}
	if v, _ := device.Sessions.GetSession(ctx, lidAddrs[0]); !reflect.DeepEqual(v, sec.sessionA) {
		t.Errorf("the migrated session does not open under the LID address")
	}
	var plainRows int
	must(t, db.QueryRow(`SELECT COUNT(*) FROM whatsmeow_sessions WHERE their_id LIKE '15551230002:%' AND their_id != '15551230002:5'`).Scan(&plainRows))
	if plainRows != 0 {
		t.Errorf("%d PN session rows are left after the migration", plainRows)
	}
}

// The pinned exported method set of whatsmeow's *sqlstore.SQLStore, the type sealedStore wraps.
// A method that is not overridden falls through to SQLStore and writes what it gets in plain, so
// a whatsmeow bump that adds, removes or changes one must be reviewed: decide whether it touches
// a sealed column (S6), override it in sealedstore.go if it does, then update this list.
var pinnedSQLStoreMethods = []string{
	"AddOutgoingEvent func(*sqlstore.SQLStore, context.Context, types.JID, string, string, []uint8) error",
	"ClearBufferedEventPlaintext func(*sqlstore.SQLStore, context.Context, [32]uint8) error",
	"Close func(*sqlstore.SQLStore) error",
	"DeleteAllIdentities func(*sqlstore.SQLStore, context.Context, string) error",
	"DeleteAllSessions func(*sqlstore.SQLStore, context.Context, string) error",
	"DeleteAppStateMutationMACs func(*sqlstore.SQLStore, context.Context, string, [][]uint8) error",
	"DeleteAppStateVersion func(*sqlstore.SQLStore, context.Context, string) error",
	"DeleteDevice func(*sqlstore.SQLStore, context.Context, *store.Device) error",
	"DeleteExpiredPrivacyTokens func(*sqlstore.SQLStore, context.Context, time.Time) (int64, error)",
	"DeleteIdentity func(*sqlstore.SQLStore, context.Context, string) error",
	"DeleteNCTSalt func(*sqlstore.SQLStore, context.Context) error",
	"DeleteOldBufferedHashes func(*sqlstore.SQLStore, context.Context) error",
	"DeleteOldOutgoingEvents func(*sqlstore.SQLStore, context.Context) error",
	"DeleteSession func(*sqlstore.SQLStore, context.Context, string) error",
	"DoDecryptionTxn func(*sqlstore.SQLStore, context.Context, func(context.Context) error) error",
	"GenOnePreKey func(*sqlstore.SQLStore, context.Context) (*keys.PreKey, error)",
	"GetAllAppStateSyncKeys func(*sqlstore.SQLStore, context.Context) ([]*store.AppStateSyncKey, error)",
	"GetAllContacts func(*sqlstore.SQLStore, context.Context) (map[types.JID]types.ContactInfo, error)",
	"GetAllDevices func(*sqlstore.SQLStore, context.Context) ([]*store.Device, error)",
	"GetAppStateMutationMAC func(*sqlstore.SQLStore, context.Context, string, []uint8) ([]uint8, error)",
	"GetAppStateSyncKey func(*sqlstore.SQLStore, context.Context, []uint8) (*store.AppStateSyncKey, error)",
	"GetAppStateVersion func(*sqlstore.SQLStore, context.Context, string) (uint64, [128]uint8, error)",
	"GetBufferedEvent func(*sqlstore.SQLStore, context.Context, [32]uint8) (*store.BufferedEvent, error)",
	"GetChatSettings func(*sqlstore.SQLStore, context.Context, types.JID) (types.LocalChatSettings, error)",
	"GetContact func(*sqlstore.SQLStore, context.Context, types.JID) (types.ContactInfo, error)",
	"GetDevice func(*sqlstore.SQLStore, context.Context, types.JID) (*store.Device, error)",
	"GetFirstDevice func(*sqlstore.SQLStore, context.Context) (*store.Device, error)",
	"GetLatestAppStateSyncKeyID func(*sqlstore.SQLStore, context.Context) ([]uint8, error)",
	"GetManySessions func(*sqlstore.SQLStore, context.Context, []string) (map[string][]uint8, error)",
	"GetMessageSecret func(*sqlstore.SQLStore, context.Context, types.JID, types.JID, string) ([]uint8, types.JID, error)",
	"GetNCTSalt func(*sqlstore.SQLStore, context.Context) ([]uint8, error)",
	"GetOrGenPreKeys func(*sqlstore.SQLStore, context.Context, uint32) ([]*keys.PreKey, error)",
	"GetOutgoingEvent func(*sqlstore.SQLStore, context.Context, types.JID, types.JID, string) (string, []uint8, error)",
	"GetPreKey func(*sqlstore.SQLStore, context.Context, uint32) (*keys.PreKey, error)",
	"GetPrivacyToken func(*sqlstore.SQLStore, context.Context, types.JID) (*store.PrivacyToken, error)",
	"GetSenderKey func(*sqlstore.SQLStore, context.Context, string, string) ([]uint8, error)",
	"GetSession func(*sqlstore.SQLStore, context.Context, string) ([]uint8, error)",
	"GetWASARootSecretID func(*sqlstore.SQLStore, context.Context, types.JID) (string, error)",
	"HasSession func(*sqlstore.SQLStore, context.Context, string) (bool, error)",
	"IsTrustedIdentity func(*sqlstore.SQLStore, context.Context, string, [32]uint8) (bool, error)",
	"MarkPreKeysAsUploaded func(*sqlstore.SQLStore, context.Context, uint32) error",
	"MigratePNToLID func(*sqlstore.SQLStore, context.Context, types.JID, types.JID) error",
	"NewDevice func(*sqlstore.SQLStore) *store.Device",
	"PutAllContactNames func(*sqlstore.SQLStore, context.Context, []store.ContactEntry) error",
	"PutAppStateMutationMACs func(*sqlstore.SQLStore, context.Context, string, uint64, []store.AppStateMutationMAC) error",
	"PutAppStateSyncKey func(*sqlstore.SQLStore, context.Context, []uint8, store.AppStateSyncKey) error",
	"PutAppStateVersion func(*sqlstore.SQLStore, context.Context, string, uint64, [128]uint8) error",
	"PutArchived func(*sqlstore.SQLStore, context.Context, types.JID, bool) error",
	"PutBufferedEvent func(*sqlstore.SQLStore, context.Context, [32]uint8, []uint8, time.Time) error",
	"PutBusinessName func(*sqlstore.SQLStore, context.Context, types.JID, string) (bool, string, error)",
	"PutContactName func(*sqlstore.SQLStore, context.Context, types.JID, string, string) error",
	"PutDevice func(*sqlstore.SQLStore, context.Context, *store.Device) error",
	"PutIdentity func(*sqlstore.SQLStore, context.Context, string, [32]uint8) error",
	"PutManyRedactedPhones func(*sqlstore.SQLStore, context.Context, []store.RedactedPhoneEntry) error",
	"PutManySessions func(*sqlstore.SQLStore, context.Context, map[string][]uint8) error",
	"PutMessageSecret func(*sqlstore.SQLStore, context.Context, types.JID, types.JID, string, []uint8) error",
	"PutMessageSecrets func(*sqlstore.SQLStore, context.Context, []store.MessageSecretInsert) error",
	"PutMutedUntil func(*sqlstore.SQLStore, context.Context, types.JID, time.Time) error",
	"PutNCTSalt func(*sqlstore.SQLStore, context.Context, []uint8) error",
	"PutPinned func(*sqlstore.SQLStore, context.Context, types.JID, bool) error",
	"PutPrivacyTokens func(*sqlstore.SQLStore, context.Context, ...store.PrivacyToken) error",
	"PutPushName func(*sqlstore.SQLStore, context.Context, types.JID, string) (bool, string, error)",
	"PutSenderKey func(*sqlstore.SQLStore, context.Context, string, string, []uint8) error",
	"PutSession func(*sqlstore.SQLStore, context.Context, string, []uint8) error",
	"PutWASARootSecretID func(*sqlstore.SQLStore, context.Context, types.JID, string) error",
	"RemovePreKey func(*sqlstore.SQLStore, context.Context, uint32) error",
	"Upgrade func(*sqlstore.SQLStore, context.Context) error",
	"UploadedPreKeyCount func(*sqlstore.SQLStore, context.Context) (int, error)",
}

func TestSQLStoreMethodSetIsPinned(t *testing.T) {
	typ := reflect.TypeOf((*sqlstore.SQLStore)(nil))
	var got []string
	for i := range typ.NumMethod() {
		m := typ.Method(i)
		got = append(got, m.Name+" "+m.Type.String())
	}
	slices.Sort(got)
	want := slices.Clone(pinnedSQLStoreMethods)
	slices.Sort(want)
	if slices.Equal(got, want) {
		return
	}
	var added, removed []string
	for _, m := range got {
		if !slices.Contains(want, m) {
			added = append(added, m)
		}
	}
	for _, m := range want {
		if !slices.Contains(got, m) {
			removed = append(removed, m)
		}
	}
	t.Errorf("whatsmeow's *sqlstore.SQLStore method set changed (a whatsmeow bump?).\n"+
		"Review each method below: if it reads or writes a sealed column (ADR 0003, S6), override it in "+
		"internal/store/sealedstore.go; then update pinnedSQLStoreMethods (docs/bump-drill.md).\n"+
		"added or changed:\n\t%s\nremoved or changed:\n\t%s",
		strings.Join(added, "\n\t"), strings.Join(removed, "\n\t"))
}

// errText is an error's message, or "" for nil, so two stores' errors compare by value.
func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// A nil value in a NOT NULL sealed column is stored as SQL NULL, so the write fails exactly as it
// does on the stock store, and reading or expiring afterwards sees what the stock store sees.
func TestSealedStoreRefusesANilSecretLikeTheStockStore(t *testing.T) {
	ctx := context.Background()
	cases := []struct {
		name string
		run  func(d *wastore.Device) []observed
	}{
		{"PutSenderKey", func(d *wastore.Device) []observed {
			put := d.SenderKeys.PutSenderKey(ctx, testGroup, addrA, nil)
			got, err := d.SenderKeys.GetSenderKey(ctx, testGroup, addrA)
			return []observed{{"put", errText(put)}, {"get", []any{got, errText(err)}}}
		}},
		{"PutPrivacyTokens", func(d *wastore.Device) []observed {
			put := d.PrivacyTokens.PutPrivacyTokens(ctx, wastore.PrivacyToken{User: peerPN, Timestamp: time.Unix(1000, 0)})
			got, err := d.PrivacyTokens.GetPrivacyToken(ctx, peerPN)
			n, delErr := d.PrivacyTokens.DeleteExpiredPrivacyTokens(ctx, time.Unix(2000, 0))
			return []observed{{"put", errText(put)}, {"get", []any{got, errText(err)}}, {"expired", []any{n, errText(delErr)}}}
		}},
		{"PutNCTSalt", func(d *wastore.Device) []observed {
			put := d.NCTSalt.PutNCTSalt(ctx, nil)
			got, err := d.NCTSalt.GetNCTSalt(ctx)
			return []observed{{"put", errText(put)}, {"get", []any{got, errText(err)}}}
		}},
		{"AddOutgoingEvent", func(d *wastore.Device) []observed {
			put := d.EventBuffer.AddOutgoingEvent(ctx, peerPN, "MSGID9", "proto", nil)
			format, got, err := d.EventBuffer.GetOutgoingEvent(ctx, peerPN, peerLID, "MSGID9")
			return []observed{{"put", errText(put)}, {"get", []any{format, got, errText(err)}}}
		}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			device, _, _ := sealedDevice(t, t.TempDir())
			stock, _ := stockDevice(t, t.TempDir(), device)
			got, want := c.run(device), c.run(stock)
			if want[0].value == "" {
				t.Fatalf("the stock store accepted a nil value; this test assumes it fails on NOT NULL")
			}
			if !reflect.DeepEqual(got, want) {
				t.Errorf("sealed store %#v\nstock store  %#v", got, want)
			}
		})
	}
}

// keptContainerMethods are the *sqlstore.Container methods sealedStore promotes on purpose:
// neither reads nor writes a whatsmeow_device row. Every other one is shadowed to fail, because
// the device's keys belong to DeviceStore and the Container's own methods would store them plain.
var keptContainerMethods = []string{"Close", "Upgrade"}

// containerMethodError calls m with args built from its signature and returns the error it
// reports: its last result when that is an error, or the value it panics with.
func containerMethodError(m reflect.Value, args []reflect.Value) (err error) {
	defer func() {
		if r := recover(); r != nil {
			if e, ok := r.(error); ok {
				err = e
			} else {
				err = fmt.Errorf("panic: %v", r)
			}
		}
	}()
	out := m.Call(args)
	if len(out) > 0 {
		if e, ok := out[len(out)-1].Interface().(error); ok {
			return e
		}
	}
	return nil
}

// Each Container method sealedStore would promote, other than the kept ones, fails with
// errDevicesBelongToDeviceStore and writes nothing. It walks Container's method set, so a method
// a whatsmeow bump adds is called too and fails this test until it is shadowed or kept.
func TestSealedStoreRefusesTheContainersDeviceMethods(t *testing.T) {
	ctx := context.Background()
	typ := reflect.TypeOf((*sqlstore.Container)(nil))
	for _, kept := range keptContainerMethods {
		if _, ok := typ.MethodByName(kept); !ok {
			t.Errorf("keptContainerMethods names %s, which *sqlstore.Container no longer has", kept)
		}
	}
	ctxType := reflect.TypeOf((*context.Context)(nil)).Elem()
	for i := range typ.NumMethod() {
		name := typ.Method(i).Name
		if slices.Contains(keptContainerMethods, name) {
			continue
		}
		t.Run(name, func(t *testing.T) {
			device, st, db := sealedDevice(t, t.TempDir())
			// A second device, so a stock PutDevice would add its row in plain.
			other := pairedDevice(t, types.JID{User: "15551230009", Server: types.DefaultUserServer, Device: 3})
			before := dbHash(t, db)
			m := reflect.ValueOf(st).MethodByName(name)
			args := make([]reflect.Value, m.Type().NumIn())
			for j := range args {
				switch in := m.Type().In(j); {
				case in == ctxType:
					args[j] = reflect.ValueOf(ctx)
				case in == reflect.TypeOf(device) && name == "DeleteDevice":
					args[j] = reflect.ValueOf(device)
				case in == reflect.TypeOf(device):
					args[j] = reflect.ValueOf(other)
				case in == reflect.TypeOf(types.JID{}):
					args[j] = reflect.ValueOf(*device.ID)
				default:
					args[j] = reflect.Zero(in)
				}
			}
			if err := containerMethodError(m, args); !errors.Is(err, errDevicesBelongToDeviceStore) {
				t.Errorf("sealedStore.%s reported %v, want errDevicesBelongToDeviceStore", name, err)
			}
			if after := dbHash(t, db); after != before {
				t.Errorf("sealedStore.%s changed the database", name)
			}
		})
	}
}
