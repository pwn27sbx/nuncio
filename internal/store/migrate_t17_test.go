package store

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	wastore "go.mau.fi/whatsmeow/store"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
)

// The migration tests run only on databases built here, in t.TempDir, by whatsmeow's own store
// with synthetic keys, and only with the fake keyring (S3).

// plainFixture is a nuncio.db as whatsmeow's stock store leaves it after a realistic workload.
type plainFixture struct {
	dir, path string
	db        *sql.DB
	device    *wastore.Device
	// secrets is every private value the workload wrote, the overwritten, cleared and removed
	// ones included; live is the subset the stock files must still hold (the control).
	secrets, live map[string][]byte
}

// migHash is an event-buffer row that keeps its plaintext (the workload clears its own).
var migHash = [32]byte{4}

func newPlainFixture(t *testing.T) *plainFixture {
	t.Helper()
	ctx := context.Background()
	dir := t.TempDir()
	device := pairedDevice(t, ourTestJID)
	stock, db := stockDevice(t, dir, device)
	sec := newTestSecrets(t)
	preKeys := map[string][]byte{}
	exercise(t, stock, sec, preKeys)
	// Sessions that were deleted again leave whole free pages behind.
	deleted := map[string][]byte{}
	for i := range 64 {
		addr := fmt.Sprintf("1555124%04d:0", i)
		deleted["deleted session "+addr] = randBytes(t, 256)
		must(t, stock.Sessions.PutSession(ctx, addr, deleted["deleted session "+addr]))
	}
	for addr := range deleted {
		must(t, stock.Sessions.DeleteSession(ctx, strings.TrimPrefix(addr, "deleted session ")))
	}
	// Like a database that has been running for a while: what the workload overwrote, cleared or
	// removed is checkpointed into nuncio.db's free space, and the last write is still in the WAL.
	var busy, logPages, done int
	must(t, db.QueryRow(`PRAGMA wal_checkpoint(TRUNCATE)`).Scan(&busy, &logPages, &done))
	kept := randBytes(t, 40)
	must(t, stock.EventBuffer.PutBufferedEvent(ctx, migHash, kept, time.Unix(3100, 0)))

	secrets := sec.named()
	secrets["kept buffered event"] = kept
	for name, value := range devicePrivate(device) {
		secrets[name] = value
	}
	for name, value := range preKeys {
		secrets[name] = value
	}
	// Control: the deleted sessions are still in nuncio.db's free pages, so the scan after the
	// migration can fail for them.
	data, err := os.ReadFile(filepath.Join(dir, DBFile))
	must(t, err)
	inFreePages := 0
	for name, value := range deleted {
		secrets[name] = value
		if bytes.Contains(data, value) {
			inFreePages++
		}
	}
	if inFreePages == 0 {
		t.Fatal("control: no deleted session is left in nuncio.db, so the scan proves nothing for them")
	}
	live := map[string][]byte{}
	for name, value := range secrets {
		switch name {
		case "stale app-state key", "buffered event", "session A", "pre-key 2":
			// Never stored, cleared, overwritten or removed by the workload.
		default:
			if !strings.HasPrefix(name, "deleted session ") {
				live[name] = value
			}
		}
	}
	return &plainFixture{dir: dir, path: filepath.Join(dir, DBFile), db: db, device: device, secrets: secrets, live: live}
}

// fixClock makes the migration's clock return now.
func fixClock(t *testing.T, now time.Time) {
	t.Helper()
	saved := migrateNow
	migrateNow = func() time.Time { return now }
	t.Cleanup(func() { migrateNow = saved })
}

// injectFault makes the migration call fault at each of its stages.
func injectFault(t *testing.T, fault func(stage string) error) {
	t.Helper()
	saved := migrateFault
	migrateFault = fault
	t.Cleanup(func() { migrateFault = saved })
}

func backupName(path string, now time.Time) string {
	return path + ".pre-t17-" + now.UTC().Format("20060102T150405Z") + ".bak"
}

func numberedBackupName(path string, now time.Time, n int) string {
	return fmt.Sprintf("%s.pre-t17-%s-%d.bak", path, now.UTC().Format("20060102T150405Z"), n)
}

// backups lists the migration backups next to path.
func backups(t *testing.T, path string) []string {
	t.Helper()
	found, err := filepath.Glob(path + ".pre-t17-*.bak")
	if err != nil {
		t.Fatal(err)
	}
	return found
}

// openReadOnly opens a database file without writing to it or creating sidecar files.
func openReadOnly(t *testing.T, path string) *sql.DB {
	t.Helper()
	u := url.URL{Scheme: "file", Path: path, RawQuery: "mode=ro"}
	db, err := sql.Open(driverName, u.String())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = db.Close() })
	return db
}

// logicalDump is every table's schema and rows, rows sorted, so two databases with the same
// content compare equal whatever their page layout. skip names whole tables ("t"), table
// prefixes ("t*") or columns ("t.c") to leave out.
func logicalDump(t *testing.T, db *sql.DB, skip ...string) map[string][]string {
	t.Helper()
	skipped := func(name string) bool {
		for _, s := range skip {
			if s == name || (strings.HasSuffix(s, "*") && strings.HasPrefix(name, strings.TrimSuffix(s, "*"))) {
				return true
			}
		}
		return false
	}
	rows, err := db.Query(`SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name`)
	if err != nil {
		t.Fatalf("list tables: %v", err)
	}
	schema := map[string]string{}
	for rows.Next() {
		var name, sqlText string
		if err := rows.Scan(&name, &sqlText); err != nil {
			t.Fatal(err)
		}
		if !skipped(name) {
			schema[name] = sqlText
		}
	}
	_ = rows.Close()
	out := map[string][]string{}
	for name, sqlText := range schema {
		cells, err := db.Query(fmt.Sprintf(`SELECT * FROM %q`, name))
		if err != nil {
			t.Fatalf("read %s: %v", name, err)
		}
		cols, _ := cells.Columns()
		dump := []string{"schema " + sqlText}
		for cells.Next() {
			values := make([]any, len(cols))
			ptrs := make([]any, len(cols))
			for i := range values {
				ptrs[i] = &values[i]
			}
			if err := cells.Scan(ptrs...); err != nil {
				t.Fatal(err)
			}
			var row []string
			for i, v := range values {
				if skipped(name + "." + cols[i]) {
					continue
				}
				if b, ok := v.([]byte); ok {
					row = append(row, cols[i]+"=x"+hex.EncodeToString(b))
				} else {
					row = append(row, fmt.Sprintf("%s=%T:%v", cols[i], v, v))
				}
			}
			dump = append(dump, strings.Join(row, " "))
		}
		_ = cells.Close()
		slices.Sort(dump[1:])
		out[name] = dump
	}
	return out
}

// sealedColumns is every column the migration seals or zeroes, and the tables that are not
// whatsmeow's plain layout, for comparing what must stay plain.
var sealedColumns = []string{
	"whatsmeow_device.noise_key", "whatsmeow_device.identity_key", "whatsmeow_device.signed_pre_key", "whatsmeow_device.adv_key",
	"whatsmeow_device.companion_meta_nonce",
	"whatsmeow_pre_keys", "nuncio_*",
	"whatsmeow_sessions.session", "whatsmeow_sender_keys.sender_key", "whatsmeow_app_state_sync_keys.key_data",
	"whatsmeow_message_secrets.key", "whatsmeow_privacy_tokens.token", "whatsmeow_nct_salt.salt",
	"whatsmeow_event_buffer.plaintext", "whatsmeow_retry_buffer.plaintext",
}

// assertNoSecretBytes fails if any secret shows up raw, in hex or in base64 in nuncio.db, its
// -wal or its -shm.
func assertNoSecretBytes(t *testing.T, dir string, secrets map[string][]byte) {
	t.Helper()
	for file, data := range dbFiles(t, dir) {
		lower := bytes.ToLower(data)
		for name, value := range secrets {
			if len(value) == 0 {
				t.Fatalf("secret %s is empty", name)
			}
			if bytes.Contains(data, value) {
				t.Errorf("%s appears raw in %s", name, file)
			}
			if bytes.Contains(lower, []byte(hex.EncodeToString(value))) {
				t.Errorf("%s appears in hex in %s", name, file)
			}
			if bytes.Contains(data, []byte(base64.StdEncoding.EncodeToString(value))) {
				t.Errorf("%s appears in base64 in %s", name, file)
			}
		}
	}
}

// readBack reads every sealed value of the workload through the device's stores, without
// writing.
func readBack(t *testing.T, d *wastore.Device) []observed {
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
	for _, addr := range []string{addrA, addrB, addrC, "15551230005:0"} {
		v, err := d.Sessions.GetSession(ctx, addr)
		rec("GetSession "+addr, v, err)
	}
	m, err := d.Sessions.GetManySessions(ctx, []string{addrA, addrB, addrC, "15551230005:0"})
	rec("GetManySessions", m, err)
	for id := uint32(1); id <= 12; id++ {
		k, err := d.PreKeys.GetPreKey(ctx, id)
		rec(fmt.Sprintf("GetPreKey %d", id), k, err)
	}
	n, err := d.PreKeys.UploadedPreKeyCount(ctx)
	rec("UploadedPreKeyCount", n, err)
	v, err := d.SenderKeys.GetSenderKey(ctx, testGroup, addrA)
	rec("GetSenderKey", v, err)
	for _, id := range []string{"app-key-id-1", "app-key-id-2"} {
		k, err := d.AppStateKeys.GetAppStateSyncKey(ctx, []byte(id))
		rec("GetAppStateSyncKey "+id, k, err)
	}
	all, err := d.AppStateKeys.GetAllAppStateSyncKeys(ctx)
	rec("GetAllAppStateSyncKeys", all, err)
	secret, sender, err := d.MsgSecrets.GetMessageSecret(ctx, peerPN, peerPN, "MSGID1")
	rec("GetMessageSecret 1", []any{secret, sender}, err)
	secret, sender, err = d.MsgSecrets.GetMessageSecret(ctx, peerLID, peerLID, "MSGID1")
	rec("GetMessageSecret 1 through the LID", []any{secret, sender}, err)
	secret, sender, err = d.MsgSecrets.GetMessageSecret(ctx, testGroupJID, otherPeer, "MSGID2")
	rec("GetMessageSecret 2", []any{secret, sender}, err)
	for _, user := range []types.JID{peerPN, peerLID, otherPeer} {
		pt, err := d.PrivacyTokens.GetPrivacyToken(ctx, user)
		rec("GetPrivacyToken "+user.String(), pt, err)
	}
	v, err = d.NCTSalt.GetNCTSalt(ctx)
	rec("GetNCTSalt", v, err)
	for _, hash := range [][32]byte{{1}, {2}, migHash} {
		ev, err := d.EventBuffer.GetBufferedEvent(ctx, hash)
		rec(fmt.Sprintf("GetBufferedEvent %x", hash[:1]), ev, err)
	}
	format, plain, err := d.EventBuffer.GetOutgoingEvent(ctx, peerLID, peerPN, "MSGID3")
	rec("GetOutgoingEvent", []any{format, plain}, err)
	return out
}

// stockReadBack loads the device from a copy of the database file at path through whatsmeow's own
// store and reads it back, so the file itself is never written.
func stockReadBack(t *testing.T, path string) (*wastore.Device, []observed) {
	t.Helper()
	ctx := context.Background()
	dir := t.TempDir()
	copyFile(t, path, filepath.Join(dir, DBFile))
	db := openStoreDB(t, dir)
	container := sqlstore.NewWithDB(db, "sqlite3", nil)
	must(t, container.Upgrade(ctx))
	device, err := container.GetFirstDevice(ctx)
	if err != nil {
		t.Fatalf("stock GetFirstDevice: %v", err)
	}
	return device, readBack(t, device)
}

// sealedReadBack loads the device from db through the sealed device store under the key in kr.
func sealedReadBack(t *testing.T, db *sql.DB, kr Keyring) (*wastore.Device, []observed) {
	t.Helper()
	device := sealedLoad(t, db, kr)
	return device, readBack(t, device)
}

// sealedLoad loads the device from db through the sealed device store under the key in kr.
func sealedLoad(t *testing.T, db *sql.DB, kr Keyring) *wastore.Device {
	t.Helper()
	ctx := context.Background()
	key, err := LoadDataKey(kr, false)
	if err != nil {
		t.Fatalf("LoadDataKey: %v", err)
	}
	sealer, err := NewSealer(key)
	if err != nil {
		t.Fatal(err)
	}
	must(t, VerifySealCheck(ctx, db, sealer))
	device, err := newTestDeviceStore(t, db, sealer).LoadDevice(ctx)
	if err != nil {
		t.Fatalf("LoadDevice: %v", err)
	}
	return device
}

func copyFile(t *testing.T, from, to string) {
	t.Helper()
	src, err := os.Open(from)
	if err != nil {
		t.Fatal(err)
	}
	defer src.Close()
	dst, err := os.OpenFile(to, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := io.Copy(dst, src); err != nil {
		t.Fatal(err)
	}
	must(t, dst.Close())
}

func assertSameObservations(t *testing.T, got, want []observed) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("observed %d results, want %d", len(got), len(want))
	}
	for i := range got {
		if got[i].label != want[i].label || !reflect.DeepEqual(got[i].value, want[i].value) {
			t.Errorf("%s: after the migration %#v, before %#v", want[i].label, got[i].value, want[i].value)
		}
	}
}

func assertDumpsEqual(t *testing.T, got, want map[string][]string, what string) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		for name := range want {
			if !reflect.DeepEqual(got[name], want[name]) {
				t.Errorf("%s: table %s differs", what, name)
			}
		}
		for name := range got {
			if _, ok := want[name]; !ok {
				t.Errorf("%s: table %s is new", what, name)
			}
		}
	}
}

func migrate(t *testing.T, f *plainFixture, kr Keyring) MigrationResult {
	t.Helper()
	res, err := MigrateToSealed(context.Background(), f.db, f.path, kr)
	if err != nil {
		t.Fatalf("MigrateToSealed: %v", err)
	}
	return res
}

// The migration backs the database up, verified and private, then seals every private value in
// place: nothing private is left in the database files, and the sealed store reads back exactly
// what the stock store reads from the backup.
func TestMigrateBacksUpThenSealsEverything(t *testing.T) {
	f := newPlainFixture(t)
	now := time.Date(2026, 10, 21, 15, 30, 45, 0, time.FixedZone("office", 2*3600))
	fixClock(t, now)
	before := logicalDump(t, f.db)
	kr := newFakeKeyring()

	res := migrate(t, f, kr)

	if !res.Migrated || res.CleanupPending || res.Cleaned {
		t.Fatalf("result = Migrated %v, CleanupPending %v, Cleaned %v; want a finished migration", res.Migrated, res.CleanupPending, res.Cleaned)
	}
	if want := backupName(f.path, now); res.BackupPath != want || filepath.Base(want) != "nuncio.db.pre-t17-20261021T133045Z.bak" {
		t.Fatalf("BackupPath = %q, want %q", res.BackupPath, want)
	}
	assertMode(t, res.BackupPath, 0o600)
	if got := backups(t, f.path); len(got) != 1 {
		t.Fatalf("backups = %v, want one", got)
	}
	backup := openReadOnly(t, res.BackupPath)
	assertDumpsEqual(t, logicalDump(t, backup), before, "the backup against the original")
	backupData, err := os.ReadFile(res.BackupPath)
	must(t, err)
	for name, value := range f.live {
		if !bytes.Contains(backupData, value) {
			t.Errorf("control: %s is not in the backup, so the scan below proves nothing for it", name)
		}
	}
	if notice := res.Notice(); !strings.Contains(notice, res.BackupPath) || !strings.Contains(notice, "plain") {
		t.Errorf("Notice() = %q, want the backup path and that it holds the keys in plain", notice)
	}
	for _, s := range f.secrets {
		if strings.Contains(res.Notice(), hex.EncodeToString(s)) {
			t.Error("Notice() carries key material")
		}
	}
	wantRows := map[string]int{
		"whatsmeow_device": 1, "whatsmeow_pre_keys": 7, "whatsmeow_sessions": 4, "whatsmeow_sender_keys": 1,
		"whatsmeow_app_state_sync_keys": 2, "whatsmeow_message_secrets": 2, "whatsmeow_privacy_tokens": 2,
		"whatsmeow_nct_salt": 1, "whatsmeow_event_buffer": 3, "whatsmeow_retry_buffer": 1,
	}
	if !reflect.DeepEqual(res.Rows, wantRows) {
		t.Errorf("Rows = %v, want %v", res.Rows, wantRows)
	}

	// Nothing private is left in nuncio.db, -wal or -shm, open or closed; the plain columns are
	// what they were.
	assertNoSecretBytes(t, f.dir, f.secrets)
	assertDumpsEqual(t, logicalDump(t, f.db, sealedColumns...), logicalDump(t, backup, sealedColumns...), "plain columns")
	// Each sealed column keeps its NULLs (sealedStore stores a nil value as NULL) and holds
	// only values in the sealed format.
	for _, c := range migratedColumns {
		var nullsBefore, nullsAfter, notSealed int
		q := fmt.Sprintf(`SELECT COUNT(*) FROM %q WHERE %q IS NULL`, c.table, c.column)
		must(t, backup.QueryRow(q).Scan(&nullsBefore))
		must(t, f.db.QueryRow(q).Scan(&nullsAfter))
		must(t, f.db.QueryRow(fmt.Sprintf(`SELECT COUNT(*) FROM %q WHERE %q IS NOT NULL AND (length(%q) < 29 OR substr(%q, 1, 1) != x'01')`,
			c.table, c.column, c.column, c.column)).Scan(&notSealed))
		if nullsAfter != nullsBefore || notSealed != 0 {
			t.Errorf("%s.%s: %d NULLs (backup %d), %d values not sealed", c.table, c.column, nullsAfter, nullsBefore, notSealed)
		}
	}
	var preKeys int
	must(t, f.db.QueryRow(`SELECT COUNT(*) FROM whatsmeow_pre_keys`).Scan(&preKeys))
	if preKeys != 0 {
		t.Errorf("whatsmeow_pre_keys holds %d rows after the migration", preKeys)
	}
	var zeroed int
	must(t, f.db.QueryRow(`SELECT COUNT(*) FROM whatsmeow_device WHERE noise_key = zeroblob(32) AND identity_key = zeroblob(32)
		AND signed_pre_key = zeroblob(32) AND adv_key = zeroblob(32)`).Scan(&zeroed))
	if zeroed != 1 {
		t.Errorf("the whatsmeow_device row is not a zeroed stub")
	}
	var stubNonce string
	must(t, f.db.QueryRow(`SELECT companion_meta_nonce FROM whatsmeow_device`).Scan(&stubNonce))
	if stubNonce != "" {
		t.Errorf("whatsmeow_device.companion_meta_nonce is not empty after the migration")
	}

	// Differential: the sealed store on the migrated file reads what the stock store reads on
	// the backup.
	stockDev, want := stockReadBack(t, res.BackupPath)
	sealedDev, got := sealedReadBack(t, f.db, kr)
	assertSameDevice(t, sealedDev, stockDev)
	assertSameDevice(t, sealedDev, f.device)
	assertSameObservations(t, got, want)

	must(t, f.db.Close())
	assertNoSecretBytes(t, f.dir, f.secrets)
}

// A second run after a migration whose cleanup finished finds nothing to do: no second backup, no
// cleanup, no change to the database file or the keyring.
func TestMigrateTwiceChangesNothingTheSecondTime(t *testing.T) {
	f := newPlainFixture(t)
	fixClock(t, time.Date(2026, 10, 21, 10, 0, 0, 0, time.UTC))
	kr := newFakeKeyring()
	migrate(t, f, kr)
	after := logicalDump(t, f.db)
	key, err := kr.Get(KeyringService, itemDataKey)
	must(t, err)

	dbBefore, err := os.ReadFile(f.path)
	must(t, err)
	var stages []string
	injectFault(t, func(stage string) error { stages = append(stages, stage); return nil })

	fixClock(t, time.Date(2026, 10, 21, 10, 0, 5, 0, time.UTC))
	res := migrate(t, f, kr)

	if res.Migrated || res.BackupPath != "" || len(res.Rows) != 0 || res.Cleaned || res.CleanupPending {
		t.Errorf("second run = %+v, want nothing to migrate", res)
	}
	if len(stages) != 0 {
		t.Errorf("the second run reached the stages %v, want none", stages)
	}
	if dbAfter, err := os.ReadFile(f.path); err != nil || !bytes.Equal(dbAfter, dbBefore) {
		t.Errorf("the second run changed %s (%v)", DBFile, err)
	}
	if got := backups(t, f.path); len(got) != 1 {
		t.Errorf("backups after two runs = %v, want one", got)
	}
	assertDumpsEqual(t, logicalDump(t, f.db), after, "the second run")
	if again, _ := kr.Get(KeyringService, itemDataKey); again != key || kr.count() != 1 {
		t.Error("the second run changed the keyring")
	}
}

// A database without a plain device has nothing to migrate and gets no backup and no key.
func TestMigrateLeavesADatabaseWithoutAPlainDeviceAlone(t *testing.T) {
	ctx := context.Background()
	for name, build := range map[string]func(t *testing.T, dir string) *sql.DB{
		"empty": func(t *testing.T, dir string) *sql.DB { return openStoreDB(t, dir) },
		"stock tables, no device": func(t *testing.T, dir string) *sql.DB {
			db := openStoreDB(t, dir)
			must(t, sqlstore.NewWithDB(db, "sqlite3", nil).Upgrade(ctx))
			return db
		},
		"sealed": func(t *testing.T, dir string) *sql.DB {
			_, st, db := sealedDevice(t, dir)
			must(t, WriteSealCheck(ctx, db, st.sealer))
			return db
		},
	} {
		t.Run(name, func(t *testing.T) {
			dir := t.TempDir()
			db := build(t, dir)
			before := logicalDump(t, db)
			kr := newFakeKeyring()
			res, err := MigrateToSealed(ctx, db, filepath.Join(dir, DBFile), kr)
			if err != nil {
				t.Fatalf("MigrateToSealed: %v", err)
			}
			if res.Migrated || res.BackupPath != "" {
				t.Errorf("result = %+v, want nothing to migrate", res)
			}
			if got := backups(t, filepath.Join(dir, DBFile)); len(got) != 0 {
				t.Errorf("backups = %v, want none", got)
			}
			if kr.count() != 0 {
				t.Error("a data key was created with nothing to migrate")
			}
			assertDumpsEqual(t, logicalDump(t, db), before, "nothing to migrate")
		})
	}
}

// A database that has a seal check row but plain device keys is inconsistent: an error, never a
// silent reseal, and nothing is written.
func TestMigrateRefusesASealCheckOverPlainKeys(t *testing.T) {
	f := newPlainFixture(t)
	must(t, WriteSealCheck(context.Background(), f.db, testSealer(t)))
	before := logicalDump(t, f.db)
	kr := newFakeKeyring()

	_, err := MigrateToSealed(context.Background(), f.db, f.path, kr)

	if !errors.Is(err, ErrMigrationInconsistent) {
		t.Fatalf("err = %v, want ErrMigrationInconsistent", err)
	}
	assertNoSecretIn(t, err, f.device.NoiseKey.Priv[:], f.device.AdvSecretKey)
	if got := backups(t, f.path); len(got) != 0 {
		t.Errorf("backups = %v, want none", got)
	}
	if kr.count() != 0 {
		t.Error("a data key was created for an inconsistent database")
	}
	assertDumpsEqual(t, logicalDump(t, f.db), before, "an inconsistent database")
}

// A failure at any stage of the transaction leaves the database's content as it was and keeps
// the backup; a retry reuses the key the failed run created and succeeds.
func TestMigrateFailureLeavesTheOriginalAndTheRetryReusesTheKey(t *testing.T) {
	for _, stage := range []string{
		"sealed whatsmeow_sessions", "sealed whatsmeow_message_secrets", "sealed whatsmeow_retry_buffer",
		"moved whatsmeow_pre_keys", "sealed whatsmeow_device", "check",
	} {
		t.Run(stage, func(t *testing.T) {
			f := newPlainFixture(t)
			first := time.Date(2026, 10, 21, 11, 0, 0, 0, time.UTC)
			fixClock(t, first)
			before := logicalDump(t, f.db)
			kr := newFakeKeyring()
			boom := errors.New("injected failure")
			reached := false
			injectFault(t, func(s string) error {
				if s == stage {
					reached = true
					return boom
				}
				return nil
			})

			_, err := MigrateToSealed(context.Background(), f.db, f.path, kr)

			if !reached {
				t.Fatalf("the migration never reached stage %q", stage)
			}
			if !errors.Is(err, boom) {
				t.Fatalf("err = %v, want the injected failure", err)
			}
			assertDumpsEqual(t, logicalDump(t, f.db), before, "after a failed migration")
			if got := backups(t, f.path); len(got) != 1 || got[0] != backupName(f.path, first) {
				t.Fatalf("backups = %v, want the one of the failed run", got)
			}
			key, err := kr.Get(KeyringService, itemDataKey)
			if err != nil {
				t.Fatalf("the failed run left no key: %v", err)
			}

			// A1: the retry runs within the same second, so the failed run's backup name is taken.
			injectFault(t, nil)
			res := migrate(t, f, kr)
			if !res.Migrated {
				t.Fatal("the retry did not migrate")
			}
			if want := numberedBackupName(f.path, first, 1); res.BackupPath != want {
				t.Errorf("the retry's BackupPath = %q, want %q", res.BackupPath, want)
			}
			if again, _ := kr.Get(KeyringService, itemDataKey); again != key || kr.count() != 1 {
				t.Error("the retry did not reuse the key of the failed run")
			}
			if got := backups(t, f.path); len(got) != 2 {
				t.Errorf("backups = %v, want the failed run's and the retry's", got)
			}
			stockDev, want := stockReadBack(t, res.BackupPath)
			sealedDev, got := sealedReadBack(t, f.db, kr)
			assertSameDevice(t, sealedDev, stockDev)
			assertSameObservations(t, got, want)
			assertNoSecretBytes(t, f.dir, f.secrets)
		})
	}
}

// A data key already in the keyring, left by an earlier run that failed before its check row,
// is reused, never replaced.
func TestMigrateReusesADataKeyWithoutACheckRow(t *testing.T) {
	f := newPlainFixture(t)
	kr := newFakeKeyring()
	existing := base64.StdEncoding.EncodeToString(testKey(0x5a))
	must(t, kr.Set(KeyringService, itemDataKey, existing))

	migrate(t, f, kr)

	if got, _ := kr.Get(KeyringService, itemDataKey); got != existing || kr.count() != 1 {
		t.Fatal("the existing data key was replaced")
	}
	sealer, err := NewSealer(testKey(0x5a))
	must(t, err)
	must(t, VerifySealCheck(context.Background(), f.db, sealer))
}

// A keyring that fails stops the migration before anything but the backup is written.
func TestMigrateKeyringFailureWritesOnlyTheBackup(t *testing.T) {
	dbusDown := errors.New("synthetic: the secret service is not running")
	for name, kr := range map[string]func() Keyring{
		"read fails":  func() Keyring { k := newFakeKeyring(); k.getErr = dbusDown; return k },
		"write fails": func() Keyring { k := newFakeKeyring(); k.setErr = dbusDown; return k },
		"write lost":  func() Keyring { return lyingKeyring{newFakeKeyring()} },
	} {
		t.Run(name, func(t *testing.T) {
			f := newPlainFixture(t)
			before := logicalDump(t, f.db)

			_, err := MigrateToSealed(context.Background(), f.db, f.path, kr())

			if !errors.Is(err, ErrKeyringUnavailable) {
				t.Fatalf("err = %v, want ErrKeyringUnavailable", err)
			}
			assertDumpsEqual(t, logicalDump(t, f.db), before, "after a keyring failure")
			if got := backups(t, f.path); len(got) != 1 {
				t.Errorf("backups = %v, want the one written before the keyring was asked", got)
			}
		})
	}
}

// swappingKeyring replaces the data key on its n-th read, like a second process that created
// its own key at the same moment (L5 A1).
type swappingKeyring struct {
	*fakeKeyring
	n, reads int
}

func (k *swappingKeyring) Get(service, user string) (string, error) {
	k.reads++
	if k.reads == k.n {
		_ = k.fakeKeyring.Set(service, user, base64.StdEncoding.EncodeToString(testKey(0x33)))
	}
	return k.fakeKeyring.Get(service, user)
}

// After the check row is written the key is read again from the keyring; when it is not the key
// the migration sealed under, nothing is committed.
func TestMigrateRollsBackWhenTheKeyringNoLongerHoldsTheKey(t *testing.T) {
	f := newPlainFixture(t)
	before := logicalDump(t, f.db)
	// Read 1 finds no key, read 2 is LoadDataKey's read-back, read 3 is the check after the
	// check row.
	kr := &swappingKeyring{fakeKeyring: newFakeKeyring(), n: 3}

	_, err := MigrateToSealed(context.Background(), f.db, f.path, kr)

	if !errors.Is(err, ErrKeyringUnavailable) {
		t.Fatalf("err = %v, want ErrKeyringUnavailable", err)
	}
	if kr.reads < 3 {
		t.Fatalf("the keyring was read %d times, want the check after the check row", kr.reads)
	}
	assertDumpsEqual(t, logicalDump(t, f.db), before, "after the key changed under the migration")
}

// An existing file with the backup's name is never overwritten: the backup takes the next free
// numbered name instead (A1).
func TestMigrateNeverOverwritesAnExistingBackup(t *testing.T) {
	f := newPlainFixture(t)
	now := time.Date(2026, 10, 21, 12, 0, 0, 0, time.UTC)
	fixClock(t, now)
	taken := []string{backupName(f.path, now), numberedBackupName(f.path, now, 1)}
	for _, name := range taken {
		must(t, os.WriteFile(name, []byte("someone else's file"), 0o600))
	}

	res := migrate(t, f, newFakeKeyring())

	if want := numberedBackupName(f.path, now, 2); res.BackupPath != want {
		t.Errorf("BackupPath = %q, want %q", res.BackupPath, want)
	}
	for _, name := range taken {
		if data, _ := os.ReadFile(name); string(data) != "someone else's file" {
			t.Errorf("the existing file %s was overwritten", filepath.Base(name))
		}
	}
	assertMode(t, res.BackupPath, 0o600)
	stockReadBack(t, res.BackupPath)
}

// When every backup name is taken the migration refuses with ErrBackupExists and writes nothing.
func TestMigrateRefusesWhenEveryBackupNameIsTaken(t *testing.T) {
	f := newPlainFixture(t)
	now := time.Date(2026, 10, 21, 12, 30, 0, 0, time.UTC)
	fixClock(t, now)
	must(t, os.WriteFile(backupName(f.path, now), []byte("taken"), 0o600))
	for n := 1; n <= maxBackupNumber; n++ {
		must(t, os.WriteFile(numberedBackupName(f.path, now, n), []byte("taken"), 0o600))
	}
	before := logicalDump(t, f.db)
	kr := newFakeKeyring()

	_, err := MigrateToSealed(context.Background(), f.db, f.path, kr)

	if !errors.Is(err, ErrBackupExists) {
		t.Fatalf("err = %v, want ErrBackupExists", err)
	}
	if got := backups(t, f.path); len(got) != maxBackupNumber+1 {
		t.Errorf("backups = %d files, want only the %d taken ones", len(got), maxBackupNumber+1)
	}
	if kr.count() != 0 {
		t.Error("a data key was created without a backup")
	}
	assertDumpsEqual(t, logicalDump(t, f.db), before, "with every backup name taken")
}

// A backup that does not verify stops the migration before the key or the database are
// touched, and the backup file stays.
func TestMigrateStopsOnABackupThatDoesNotVerify(t *testing.T) {
	f := newPlainFixture(t)
	now := time.Date(2026, 10, 21, 13, 0, 0, 0, time.UTC)
	fixClock(t, now)
	before := logicalDump(t, f.db)
	kr := newFakeKeyring()
	injectFault(t, func(stage string) error {
		if stage == "backup" {
			// Keep the header, lose every page after the first: the device row is gone.
			return os.Truncate(backupName(f.path, now), 4096)
		}
		return nil
	})

	_, err := MigrateToSealed(context.Background(), f.db, f.path, kr)

	if err == nil {
		t.Fatal("a truncated backup was accepted")
	}
	if _, statErr := os.Stat(backupName(f.path, now)); statErr != nil {
		t.Errorf("the backup is gone: %v", statErr)
	}
	if kr.count() != 0 {
		t.Error("a data key was created after a backup that did not verify")
	}
	assertDumpsEqual(t, logicalDump(t, f.db), before, "after a backup that did not verify")
}

// secretsFound counts the secrets that appear raw in nuncio.db, -wal or -shm: the control that a
// scan after the cleanup can fail.
func secretsFound(t *testing.T, dir string, secrets map[string][]byte) int {
	t.Helper()
	found := 0
	for _, data := range dbFiles(t, dir) {
		for _, value := range secrets {
			if bytes.Contains(data, value) {
				found++
			}
		}
	}
	return found
}

// B1: a reader holding a snapshot blocks the checkpoint after the commit, so the free pages keep
// plain keys. The run fails with the cleanup pending, and the next run, with the reader gone,
// wipes them although there is nothing left to migrate.
func TestMigrateFinishesTheCleanupAReaderBlocked(t *testing.T) {
	ctx := context.Background()
	f := newPlainFixture(t)
	kr := newFakeKeyring()
	reader, err := sql.Open(driverName, f.path)
	must(t, err)
	t.Cleanup(func() { _ = reader.Close() })
	snapshot, err := reader.Conn(ctx)
	must(t, err)
	_, err = snapshot.ExecContext(ctx, `BEGIN`)
	must(t, err)
	var n int
	must(t, snapshot.QueryRowContext(ctx, `SELECT COUNT(*) FROM whatsmeow_device`).Scan(&n))

	res, err := MigrateToSealed(ctx, f.db, f.path, kr)

	if err == nil || !res.Migrated || !res.CleanupPending || res.Cleaned {
		t.Fatalf("with a reader: %+v, %v; want an error, Migrated and the cleanup pending", res, err)
	}
	assertNoSecretIn(t, err, f.device.NoiseKey.Priv[:], f.device.AdvSecretKey)
	_, err = snapshot.ExecContext(ctx, `ROLLBACK`)
	must(t, err)
	must(t, snapshot.Close())
	must(t, reader.Close())
	if secretsFound(t, f.dir, f.secrets) == 0 {
		t.Fatal("control: no plain secret is left after the blocked cleanup, so the scan below proves nothing")
	}

	res, err = MigrateToSealed(ctx, f.db, f.path, kr)

	if err != nil || res.Migrated || !res.Cleaned || res.CleanupPending {
		t.Fatalf("the next run: %+v, %v; want the cleanup done and nothing migrated", res, err)
	}
	assertNoSecretBytes(t, f.dir, f.secrets)
	sealedReadBack(t, f.db, kr)
	if res := migrate(t, f, kr); res.Cleaned || res.CleanupPending || res.Migrated {
		t.Errorf("the run after the cleanup = %+v, want nothing to do", res)
	}
	must(t, f.db.Close())
	assertNoSecretBytes(t, f.dir, f.secrets)
}

// B1: a context cancelled in the middle of the cleanup leaves it pending; a retry finishes it.
func TestMigrateRetriesACleanupCancelledHalfway(t *testing.T) {
	f := newPlainFixture(t)
	kr := newFakeKeyring()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	reached := false
	injectFault(t, func(stage string) error {
		if stage == "checkpointed" {
			reached = true
			cancel()
		}
		return nil
	})

	res, err := MigrateToSealed(ctx, f.db, f.path, kr)

	if !reached {
		t.Fatal("the cleanup never reached its first checkpoint")
	}
	if !errors.Is(err, context.Canceled) || !res.Migrated || !res.CleanupPending {
		t.Fatalf("cancelled: %+v, %v; want context.Canceled, Migrated and the cleanup pending", res, err)
	}
	if secretsFound(t, f.dir, f.secrets) == 0 {
		t.Fatal("control: no plain secret is left after the cancelled cleanup, so the scan below proves nothing")
	}

	injectFault(t, nil)
	res = migrate(t, f, kr)

	if res.Migrated || !res.Cleaned || res.CleanupPending {
		t.Fatalf("the retry = %+v, want the cleanup done", res)
	}
	assertNoSecretBytes(t, f.dir, f.secrets)
	sealedReadBack(t, f.db, kr)
}

// A3: a database with an older whatsmeow schema is refused before the backup and before the data
// key, with nothing written; the migration never upgrades the schema itself.
func TestMigrateRefusesAnOlderWhatsmeowSchema(t *testing.T) {
	for name, older := range map[string]string{
		"a table missing":  `DROP TABLE whatsmeow_nct_salt`,
		"a column missing": `ALTER TABLE whatsmeow_privacy_tokens RENAME COLUMN token TO old_token`,
		"no nonce column":  `ALTER TABLE whatsmeow_device DROP COLUMN companion_meta_nonce`,
	} {
		t.Run(name, func(t *testing.T) {
			f := newPlainFixture(t)
			_, err := f.db.Exec(older)
			must(t, err)
			before := logicalDump(t, f.db)
			kr := newFakeKeyring()

			_, err = MigrateToSealed(context.Background(), f.db, f.path, kr)

			if !errors.Is(err, ErrSchemaOutdated) {
				t.Fatalf("err = %v, want ErrSchemaOutdated", err)
			}
			if got := backups(t, f.path); len(got) != 0 {
				t.Errorf("backups = %v, want none", got)
			}
			if kr.count() != 0 {
				t.Error("a data key was created for an older schema")
			}
			assertDumpsEqual(t, logicalDump(t, f.db), before, "with an older schema")
		})
	}
}

// nonceFixture is a database sealed by the code before D3 (T17 L11): the device sealed in
// nuncio_device and a seal check row, but the companion meta nonce still plain in the
// whatsmeow_device stub row. legacy also drops nuncio_device's companion_meta_nonce column, as
// that code created the table without it.
type nonceFixture struct {
	dir, path string
	db        *sql.DB
	kr        *fakeKeyring
	device    *wastore.Device
	nonce     string
}

func newNonceFixture(t *testing.T, legacy bool) *nonceFixture {
	t.Helper()
	ctx := context.Background()
	dir := t.TempDir()
	db := openStoreDB(t, dir)
	key := randBytes(t, dataKeySize)
	kr := newFakeKeyring()
	must(t, kr.Set(KeyringService, itemDataKey, base64.StdEncoding.EncodeToString(key)))
	sealer, err := NewSealer(key)
	must(t, err)
	device := pairedDevice(t, ourTestJID)
	device.Container = newTestDeviceStore(t, db, sealer)
	must(t, device.Save(ctx))
	must(t, WriteSealCheck(ctx, db, sealer))
	_, err = db.Exec(`UPDATE whatsmeow_device SET companion_meta_nonce = ?`, device.CompanionMetaNonce)
	must(t, err)
	_, err = db.Exec(`UPDATE nuncio_device SET companion_meta_nonce = NULL`)
	must(t, err)
	if legacy {
		_, err = db.Exec(`ALTER TABLE nuncio_device DROP COLUMN companion_meta_nonce`)
		must(t, err)
	}
	var busy, logPages, done int
	must(t, db.QueryRow(`PRAGMA wal_checkpoint(TRUNCATE)`).Scan(&busy, &logPages, &done))
	f := &nonceFixture{dir: dir, path: filepath.Join(dir, DBFile), db: db, kr: kr, device: device, nonce: device.CompanionMetaNonce}
	if secretsFound(t, dir, f.secrets()) == 0 {
		t.Fatal("control: the plain nonce is not in the database files, so the scan proves nothing")
	}
	return f
}

func (f *nonceFixture) secrets() map[string][]byte {
	return map[string][]byte{"companion meta nonce": []byte(f.nonce)}
}

// D3: a database sealed before the nonce was gets it sealed on the next MigrateToSealed, after a
// backup like any migration, in one transaction with the cleanup that wipes the free pages; the
// device then loads with its nonce, and a second run does nothing.
func TestMigrateSealsAPlainCompanionMetaNonceOfASealedDatabase(t *testing.T) {
	for name, legacy := range map[string]bool{"table without the column": true, "table with the column": false} {
		t.Run(name, func(t *testing.T) {
			f := newNonceFixture(t, legacy)
			now := time.Date(2026, 10, 22, 9, 0, 0, 0, time.UTC)
			fixClock(t, now)
			skip := []string{"nuncio_device", "nuncio_seal_cleanup", "whatsmeow_device.companion_meta_nonce"}
			before := logicalDump(t, f.db, skip...)

			res, err := MigrateToSealed(context.Background(), f.db, f.path, f.kr)

			if err != nil || !res.Migrated || res.CleanupPending {
				t.Fatalf("MigrateToSealed = %+v, %v; want a finished migration", res, err)
			}
			if res.BackupPath != backupName(f.path, now) {
				t.Errorf("BackupPath = %q, want %q", res.BackupPath, backupName(f.path, now))
			}
			if want := map[string]int{"whatsmeow_device": 1}; !reflect.DeepEqual(res.Rows, want) {
				t.Errorf("Rows = %v, want %v", res.Rows, want)
			}
			assertMode(t, res.BackupPath, 0o600)
			if data, err := os.ReadFile(res.BackupPath); err != nil || !bytes.Contains(data, []byte(f.nonce)) {
				t.Errorf("control: the backup does not hold the database as it was (%v)", err)
			}
			assertNoSecretBytes(t, f.dir, f.secrets())
			assertNoSecretBytes(t, f.dir, devicePrivate(f.device))
			assertDumpsEqual(t, logicalDump(t, f.db, skip...), before, "everything but the nonce")
			var stub string
			must(t, f.db.QueryRow(`SELECT companion_meta_nonce FROM whatsmeow_device`).Scan(&stub))
			if stub != "" {
				t.Error("whatsmeow_device.companion_meta_nonce is not empty after the upgrade")
			}
			loaded := sealedLoad(t, f.db, f.kr)
			assertSameDevice(t, loaded, f.device)
			if f.kr.count() != 1 {
				t.Error("the upgrade changed the keyring")
			}

			after := logicalDump(t, f.db)
			dbBefore, err := os.ReadFile(f.path)
			must(t, err)
			var stages []string
			injectFault(t, func(stage string) error { stages = append(stages, stage); return nil })
			again := migrate(t, f.asPlain(), f.kr)
			if again.Migrated || again.Cleaned || again.CleanupPending || len(stages) != 0 {
				t.Errorf("second run = %+v, stages %v; want nothing to do", again, stages)
			}
			if dbAfter, err := os.ReadFile(f.path); err != nil || !bytes.Equal(dbAfter, dbBefore) {
				t.Errorf("the second run changed %s (%v)", DBFile, err)
			}
			assertDumpsEqual(t, logicalDump(t, f.db), after, "the second run")
			if got := backups(t, f.path); len(got) != 1 {
				t.Errorf("backups = %v, want one", got)
			}
			must(t, f.db.Close())
			assertNoSecretBytes(t, f.dir, f.secrets())
		})
	}
}

// asPlain lets the plainFixture helpers run on a nonce fixture.
func (f *nonceFixture) asPlain() *plainFixture {
	return &plainFixture{dir: f.dir, path: f.path, db: f.db, device: f.device}
}

// D3: a failure inside the nonce upgrade's transaction leaves the database's content as it was
// (the legacy table keeps its old columns), keeps the backup and the key, and a retry succeeds.
func TestMigrateCompanionMetaNonceFailureLeavesTheOriginal(t *testing.T) {
	for _, stage := range []string{"sealed companion_meta_nonce", "check"} {
		t.Run(stage, func(t *testing.T) {
			f := newNonceFixture(t, true)
			before := logicalDump(t, f.db)
			key, err := f.kr.Get(KeyringService, itemDataKey)
			must(t, err)
			boom := errors.New("injected failure")
			reached := false
			injectFault(t, func(s string) error {
				if s == stage {
					reached = true
					return boom
				}
				return nil
			})

			_, err = MigrateToSealed(context.Background(), f.db, f.path, f.kr)

			if !reached || !errors.Is(err, boom) {
				t.Fatalf("reached %v, err = %v; want the injected failure at %q", reached, err, stage)
			}
			assertNoSecretIn(t, err, []byte(f.nonce))
			assertDumpsEqual(t, logicalDump(t, f.db), before, "after a failed upgrade")
			if got := backups(t, f.path); len(got) != 1 {
				t.Errorf("backups = %v, want the one of the failed run", got)
			}

			injectFault(t, nil)
			res := migrate(t, f.asPlain(), f.kr)
			if !res.Migrated || res.CleanupPending {
				t.Fatalf("the retry = %+v, want a finished migration", res)
			}
			if again, _ := f.kr.Get(KeyringService, itemDataKey); again != key || f.kr.count() != 1 {
				t.Error("the keyring changed")
			}
			loaded := sealedLoad(t, f.db, f.kr)
			assertSameDevice(t, loaded, f.device)
			assertNoSecretBytes(t, f.dir, f.secrets())
		})
	}
}

// D3: the nonce is sealed only under the key the seal check row proves. Without that key (none,
// another one, no check row) the upgrade writes nothing, no backup and no key, and leaves the
// verdict to the caller's seal check; a keyring that cannot be read is ErrKeyringUnavailable,
// also before the backup.
func TestMigrateCompanionMetaNonceNeedsTheSealingKey(t *testing.T) {
	for name, tc := range map[string]struct {
		prepare func(t *testing.T, f *nonceFixture)
		wantErr error
	}{
		"no key": {prepare: func(t *testing.T, f *nonceFixture) {
			must(t, f.kr.Delete(KeyringService, itemDataKey))
		}},
		"another key": {prepare: func(t *testing.T, f *nonceFixture) {
			must(t, f.kr.Set(KeyringService, itemDataKey, base64.StdEncoding.EncodeToString(testKey(0x77))))
		}},
		"no check row": {prepare: func(t *testing.T, f *nonceFixture) {
			_, err := f.db.Exec(`DELETE FROM nuncio_seal_meta`)
			must(t, err)
		}},
		"keyring down": {prepare: func(t *testing.T, f *nonceFixture) {
			f.kr.getErr = errors.New("synthetic: the secret service is not running")
		}, wantErr: ErrKeyringUnavailable},
	} {
		t.Run(name, func(t *testing.T) {
			f := newNonceFixture(t, true)
			tc.prepare(t, f)
			before := logicalDump(t, f.db)
			items := f.kr.count()

			res, err := MigrateToSealed(context.Background(), f.db, f.path, f.kr)

			if tc.wantErr == nil && err != nil || tc.wantErr != nil && !errors.Is(err, tc.wantErr) {
				t.Fatalf("err = %v, want %v", err, tc.wantErr)
			}
			if res.Migrated {
				t.Errorf("result = %+v, want nothing migrated", res)
			}
			if got := backups(t, f.path); len(got) != 0 {
				t.Errorf("backups = %v, want none", got)
			}
			if f.kr.count() != items {
				t.Error("the keyring changed")
			}
			assertDumpsEqual(t, logicalDump(t, f.db), before, "without the sealing key")
		})
	}
}

// D3: the nonce upgrade commits only while the keyring still holds the key it sealed under, like
// the migration (L5 A1).
func TestMigrateCompanionMetaNonceRollsBackWhenTheKeyChanges(t *testing.T) {
	f := newNonceFixture(t, true)
	before := logicalDump(t, f.db)
	// Read 1 is sealingKey's LoadDataKey, read 2 the check before the commit.
	kr := &swappingKeyring{fakeKeyring: f.kr, n: 2}

	_, err := MigrateToSealed(context.Background(), f.db, f.path, kr)

	if !errors.Is(err, ErrKeyringUnavailable) || kr.reads != 2 {
		t.Fatalf("err = %v after %d reads, want ErrKeyringUnavailable after the check before the commit", err, kr.reads)
	}
	assertDumpsEqual(t, logicalDump(t, f.db), before, "after the key changed under the upgrade")
}

// D3: a backup that does not hold the plain nonce stops the upgrade before the write lock, and
// the database stays as it was.
func TestMigrateCompanionMetaNonceStopsOnABackupWithoutTheNonce(t *testing.T) {
	f := newNonceFixture(t, true)
	now := time.Date(2026, 10, 22, 9, 30, 0, 0, time.UTC)
	fixClock(t, now)
	before := logicalDump(t, f.db)
	injectFault(t, func(stage string) error {
		if stage != "backup" {
			return nil
		}
		backup, err := sql.Open(driverName, backupName(f.path, now))
		if err != nil {
			return err
		}
		defer backup.Close()
		_, err = backup.Exec(`UPDATE whatsmeow_device SET companion_meta_nonce = 'another'`)
		return err
	})

	_, err := MigrateToSealed(context.Background(), f.db, f.path, f.kr)

	if err == nil || !strings.Contains(err.Error(), "does not hold the device") {
		t.Fatalf("err = %v, want the backup refused", err)
	}
	assertDumpsEqual(t, logicalDump(t, f.db), before, "after a backup without the nonce")
}

// D3: a cleanup interrupted after the nonce upgrade's commit stays pending, with no plain nonce
// in the files, and the next run finishes it.
func TestMigrateCompanionMetaNonceCleanupInterrupted(t *testing.T) {
	f := newNonceFixture(t, true)
	boom := errors.New("injected failure")
	injectFault(t, func(stage string) error {
		if stage == "checkpointed" {
			return boom
		}
		return nil
	})

	res, err := MigrateToSealed(context.Background(), f.db, f.path, f.kr)

	if !errors.Is(err, boom) || !res.Migrated || !res.CleanupPending {
		t.Fatalf("interrupted: %+v, %v; want the failure, Migrated and the cleanup pending", res, err)
	}
	assertNoSecretBytes(t, f.dir, f.secrets())
	injectFault(t, nil)
	res = migrate(t, f.asPlain(), f.kr)
	if res.Migrated || !res.Cleaned || res.CleanupPending {
		t.Fatalf("the retry = %+v, want the cleanup done", res)
	}
	assertNoSecretBytes(t, f.dir, f.secrets())
	if loaded := sealedLoad(t, f.db, f.kr); loaded.CompanionMetaNonce != f.nonce {
		t.Error("the device lost its nonce")
	}
}
