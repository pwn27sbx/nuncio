package store

import (
	"bytes"
	"context"
	"database/sql"
	"database/sql/driver"
	"errors"
	"fmt"
	"io/fs"
	"net/url"
	"os"
	"path/filepath"
	"time"
)

// The T17 migration (ADR 0003, 14-15): a nuncio.db written by whatsmeow's stock store, with the
// device's private keys, the pre-keys and the Signal state in plain, becomes the sealed layout
// of DeviceStore and sealedStore. It backs the database up first, seals in place in one
// transaction, and then vacuums so no freed page keeps a plain key; a durable mark makes a later
// call finish that vacuum when it was interrupted.

var (
	// ErrMigrationInconsistent reports a database with a seal check row whose device keys are
	// still in plain. It is never resealed: something other than the migration wrote it.
	ErrMigrationInconsistent = errors.New("store: the database has a seal check row but its device keys are in plain")
	// ErrBackupExists reports that a file with the backup's name is already there. It is never
	// overwritten.
	ErrBackupExists = errors.New("store: a file with the migration backup's name already exists")
	// ErrSchemaOutdated reports a database whose whatsmeow tables are older than the ones the
	// migration seals. The caller runs whatsmeow's schema upgrade (sqlstore.Container.Upgrade)
	// first; the migration never upgrades the schema itself.
	ErrSchemaOutdated = errors.New("store: the database has an older whatsmeow schema; run the whatsmeow schema upgrade before the migration")
)

// backupTimeLayout names the backup <db>.pre-t17-<UTC time>.bak, e.g.
// nuncio.db.pre-t17-20261021T133045Z.bak.
const backupTimeLayout = "20060102T150405Z"

// maxBackupNumber is how many numbered names (<db>.pre-t17-<UTC time>-1.bak up to -99) the
// backup tries after its plain name, so a retry within the same second gets its own file.
const maxBackupNumber = 99

// errMigratedMeanwhile reports that, once this call held SQLite's write lock, the device keys it
// found in plain were sealed already: another process (nunciod and `nuncio link` can both start a
// migration) committed its migration first.
var errMigratedMeanwhile = errors.New("store: another process sealed the database meanwhile")

var (
	// migrateBusyTimeout is how long a migration waits for SQLite's write lock while another
	// process holds it (its migration, or that process's keyring prompt inside it).
	migrateBusyTimeout = 30 * time.Second
	// migrateNow is the clock that names the backup. Tests replace it.
	migrateNow = time.Now
	// migrateFault, when set, is called at each stage of the migration; an error it returns
	// stops the migration there, as a real failure at that point would. Tests set it.
	migrateFault func(stage string) error
)

// MigrationResult is what MigrateToSealed did.
type MigrationResult struct {
	// Migrated is false when there was nothing to migrate; nothing was written then.
	Migrated bool
	// BackupPath is the plain copy of the database written before the migration. It holds
	// the device's keys in plain and is never deleted by the migration.
	BackupPath string
	// Rows counts the rows sealed (or, for whatsmeow_pre_keys, moved into nuncio_pre_keys)
	// per whatsmeow table.
	Rows map[string]int
	// Cleaned is true when this call wiped the free pages that an earlier, interrupted run left
	// with plain keys (its cleanup-pending mark was set).
	Cleaned bool
	// CleanupPending is true when the free pages may still hold plain keys after this call: the
	// commit happened but the cleanup did not finish. The mark stays in the database and the next
	// call retries the cleanup; the caller warns the user until then.
	CleanupPending bool
}

// Notice is the sentence the caller shows the user after a migration: where the backup is and
// that it holds the keys in plain. It carries no key material, ids or numbers.
func (r MigrationResult) Notice() string {
	if !r.Migrated {
		return ""
	}
	return fmt.Sprintf("nuncio sealed the WhatsApp keys in its database. The backup %s still holds them in plain: "+
		"delete it once nuncio works.", r.BackupPath)
}

// migratedColumn is one sealed column of whatsmeow's tables and how sealedStore binds it: the
// column holding our JID and the rest of the primary key, in sealedStore's AAD order.
type migratedColumn struct {
	table, column, ourCol string
	pk                    []string
}

// migratedColumns are the S6 columns sealedStore seals in whatsmeow's own tables, with the AAD
// parts each of its methods passes to seal. Pre-keys and the device keys move to nuncio's tables
// instead (movePreKeys, sealDevices).
var migratedColumns = []migratedColumn{
	{tSessions, "session", "our_jid", []string{"their_id"}},
	{tSenderKeys, "sender_key", "our_jid", []string{"chat_id", "sender_id"}},
	{tAppStateKey, "key_data", "jid", []string{"key_id"}},
	{tMsgSecrets, "key", "our_jid", []string{"chat_jid", "sender_jid", "message_id"}},
	{tPrivacy, "token", "our_jid", []string{"their_jid"}},
	{tNCTSalt, "salt", "our_jid", nil},
	{tEventBuffer, "plaintext", "our_jid", []string{"ciphertext_hash"}},
	{tRetryBuffer, "plaintext", "our_jid", []string{"chat_jid", "message_id"}},
}

// plainDevice is a whatsmeow_device row whose private columns hold real keys.
type plainDevice struct {
	jid                                      string
	noise, identity, signedPreKey, advSecret []byte
}

// MigrateToSealed seals a plain database in place. db is the open database at dbPath (opened
// with Open); kr holds the data key, which is created when it is not there yet and reused when
// an earlier run that failed left it.
//
// It runs only when a whatsmeow_device row holds real private keys and the database has no seal
// check row; a seal check row over plain device keys is ErrMigrationInconsistent. A whatsmeow
// schema older than the tables it seals is ErrSchemaOutdated, before anything is written.
//
// In order: a backup with VACUUM INTO at <dbPath>.pre-t17-<UTC time>.bak, or <…>-<n>.bak when
// that name is taken (0600, never over an existing file), verified read-only; SQLite's write lock
// (BEGIN IMMEDIATE), under which the plain device keys are looked for again and the data key is
// taken from the keyring or created there (LoadDataKey, which never replaces a key it finds); one
// transaction that seals every private column, moves the pre-keys to nuncio_pre_keys, seals the
// device keys into nuncio_device, zeroes whatsmeow_device's private columns and writes the seal
// check row and the cleanup-pending mark, committed only when the keyring still holds the key it
// sealed under; then, with secure_delete on, a WAL checkpoint and VACUUM wipe the free pages
// stock whatsmeow left, and only then is the mark cleared. A failure before the commit leaves
// the database's content as it was. A failure after it returns Migrated and CleanupPending; every
// later call that finds the mark retries the cleanup, even with nothing left to migrate, and
// reports Cleaned when it finishes. With nothing to migrate and no mark it returns a zero
// MigrationResult and writes nothing. The backup is never deleted. Errors carry no key material,
// ids or numbers.
//
// Two processes may migrate the same database at once (L9 B1). Every key creation happens under
// the write lock, so a second process waits until the first one committed (up to
// migrateBusyTimeout) and then finds nothing left to seal and writes no key: the keyring always
// ends holding the key the committed data is sealed under. A call that finds the database sealed
// meanwhile starts its checks again, as a call made after the other one would.
//
// Call it with no other user of db: it holds one connection for its whole run.
func MigrateToSealed(ctx context.Context, db *sql.DB, dbPath string, kr Keyring) (MigrationResult, error) {
	conn, err := db.Conn(ctx)
	if err != nil {
		return MigrationResult{}, fmt.Errorf("store: migration: take a connection: %w", err)
	}
	defer conn.Close()

	res, err := migrateOnce(ctx, conn, dbPath, kr)
	if errors.Is(err, errMigratedMeanwhile) {
		res, err = migrateOnce(ctx, conn, dbPath, kr)
	}
	return res, err
}

// busyTimeoutOn makes conn wait up to migrateBusyTimeout for SQLite's write lock while another
// process holds it, and returns the function that puts the previous timeout back.
func busyTimeoutOn(ctx context.Context, conn *sql.Conn) (restore func(), err error) {
	var prev int64
	if err := conn.QueryRowContext(ctx, `PRAGMA busy_timeout`).Scan(&prev); err != nil {
		return nil, fmt.Errorf("store: migration: read busy_timeout: %w", err)
	}
	if _, err := conn.ExecContext(ctx, fmt.Sprintf(`PRAGMA busy_timeout=%d`, migrateBusyTimeout.Milliseconds())); err != nil {
		return nil, fmt.Errorf("store: migration: set busy_timeout: %w", err)
	}
	return func() {
		_, _ = conn.ExecContext(context.WithoutCancel(ctx), fmt.Sprintf(`PRAGMA busy_timeout=%d`, prev))
	}, nil
}

// migrateOnce is one pass of MigrateToSealed on its held connection.
func migrateOnce(ctx context.Context, conn *sql.Conn, dbPath string, kr Keyring) (MigrationResult, error) {
	pending, err := cleanupPending(ctx, conn)
	if err != nil {
		return MigrationResult{}, err
	}
	devices, err := plainDevices(ctx, conn)
	if err != nil {
		return MigrationResult{}, err
	}
	defer func() {
		for _, d := range devices {
			clear(d.noise)
			clear(d.identity)
			clear(d.signedPreKey)
			clear(d.advSecret)
		}
	}()
	sealedMeta, err := hasSealCheck(ctx, conn)
	if err != nil {
		return MigrationResult{}, err
	}
	switch {
	case len(devices) == 0 && pending:
		// An earlier run committed but did not finish wiping its free pages.
		if err := cleanFreePages(ctx, conn); err != nil {
			return MigrationResult{CleanupPending: true}, err
		}
		return MigrationResult{Cleaned: true}, nil
	case len(devices) == 0:
		return MigrationResult{}, nil
	case sealedMeta:
		return MigrationResult{}, ErrMigrationInconsistent
	}
	if err := checkSchema(ctx, conn); err != nil {
		return MigrationResult{}, err
	}

	backup, err := writeBackup(ctx, conn, dbPath+".pre-t17-"+migrateNow().UTC().Format(backupTimeLayout))
	if err != nil {
		return MigrationResult{}, err
	}
	if err := fault("backup"); err != nil {
		return MigrationResult{}, err
	}
	if err := verifyBackup(ctx, backup, devices); err != nil {
		return MigrationResult{}, err
	}

	restore, err := secureDeleteOn(ctx, conn)
	if err != nil {
		return MigrationResult{}, err
	}
	rows, sealer, err := sealInPlace(ctx, conn, kr)
	restore()
	if err != nil {
		return MigrationResult{}, err
	}
	res := MigrationResult{Migrated: true, BackupPath: backup, Rows: rows, CleanupPending: true}
	if err := cleanFreePages(ctx, conn); err != nil {
		return res, err
	}
	res.CleanupPending = false
	if err := verifySealCheckConn(ctx, conn, sealer); err != nil {
		return res, fmt.Errorf("store: migration: the committed seal check does not verify: %w", err)
	}
	return res, nil
}

// secureDeleteOn turns secure_delete on for conn, so what a write frees is overwritten, and
// returns the function that puts the previous setting back.
func secureDeleteOn(ctx context.Context, conn *sql.Conn) (restore func(), err error) {
	var prev int
	if err := conn.QueryRowContext(ctx, `PRAGMA secure_delete`).Scan(&prev); err != nil {
		return nil, fmt.Errorf("store: migration: read secure_delete: %w", err)
	}
	if _, err := conn.ExecContext(ctx, `PRAGMA secure_delete=ON`); err != nil {
		return nil, fmt.Errorf("store: migration: turn on secure_delete: %w", err)
	}
	return func() {
		_, _ = conn.ExecContext(context.WithoutCancel(ctx), fmt.Sprintf(`PRAGMA secure_delete=%d`, prev))
	}, nil
}

// The cleanup-pending mark (ADR 0003, 14): the migration transaction writes this row together
// with the sealed values, and only a finished cleanFreePages deletes it. Until then the database's
// free pages may still hold the plain keys stock whatsmeow freed, so every later MigrateToSealed
// call retries the cleanup. The row holds no secret.
const cleanupMarkSchema = `
CREATE TABLE IF NOT EXISTS nuncio_seal_cleanup (
	id INTEGER PRIMARY KEY CHECK (id = 1)
)`

// cleanupPending reports whether the cleanup-pending mark is set.
func cleanupPending(ctx context.Context, conn *sql.Conn) (bool, error) {
	if ok, err := tableExists(ctx, conn, "nuncio_seal_cleanup"); err != nil || !ok {
		return false, err
	}
	var n int
	if err := conn.QueryRowContext(ctx, `SELECT COUNT(*) FROM nuncio_seal_cleanup`).Scan(&n); err != nil {
		return false, fmt.Errorf("store: migration: read nuncio_seal_cleanup: %w", err)
	}
	return n > 0, nil
}

// cleanFreePages wipes the free pages with secure_delete on and then clears the cleanup-pending
// mark. A failure anywhere leaves the mark, so the next call starts the cleanup again.
func cleanFreePages(ctx context.Context, conn *sql.Conn) error {
	restore, err := secureDeleteOn(ctx, conn)
	if err != nil {
		return err
	}
	defer restore()
	if err := wipeFreedPages(ctx, conn); err != nil {
		return err
	}
	if _, err := conn.ExecContext(ctx, `DELETE FROM nuncio_seal_cleanup`); err != nil {
		return fmt.Errorf("store: migration: clear the cleanup mark: %w", err)
	}
	return nil
}

// requiredSchema is every whatsmeow table and column the migration reads or writes.
func requiredSchema() map[string][]string {
	tables := map[string][]string{
		"whatsmeow_device":   {"jid", "noise_key", "identity_key", "signed_pre_key", "adv_key"},
		"whatsmeow_pre_keys": {"jid", "key_id", "key", "uploaded"},
	}
	for _, c := range migratedColumns {
		tables[c.table] = append([]string{c.ourCol, c.column}, c.pk...)
	}
	return tables
}

// checkSchema fails with ErrSchemaOutdated when a table or column of requiredSchema is missing.
// It only reads, so it runs before the backup and the data key.
func checkSchema(ctx context.Context, conn *sql.Conn) error {
	for table, columns := range requiredSchema() {
		rows, err := conn.QueryContext(ctx, `SELECT name FROM pragma_table_info(?)`, table)
		if err != nil {
			return fmt.Errorf("store: migration: read the schema of %s: %w", table, err)
		}
		have := map[string]bool{}
		for rows.Next() {
			var name string
			if err := rows.Scan(&name); err != nil {
				_ = rows.Close()
				return fmt.Errorf("store: migration: read the schema of %s: %w", table, err)
			}
			have[name] = true
		}
		err = rows.Err()
		_ = rows.Close()
		if err != nil {
			return fmt.Errorf("store: migration: read the schema of %s: %w", table, err)
		}
		if len(have) == 0 {
			return fmt.Errorf("%w (no table %s)", ErrSchemaOutdated, table)
		}
		for _, c := range columns {
			if !have[c] {
				return fmt.Errorf("%w (no column %s.%s)", ErrSchemaOutdated, table, c)
			}
		}
	}
	return nil
}

func fault(stage string) error {
	if migrateFault == nil {
		return nil
	}
	if err := migrateFault(stage); err != nil {
		return fmt.Errorf("store: migration stopped at %s: %w", stage, err)
	}
	return nil
}

// tableExists reports whether the database has a table called name.
func tableExists(ctx context.Context, conn *sql.Conn, name string) (bool, error) {
	var n int
	if err := conn.QueryRowContext(ctx, `SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?`, name).Scan(&n); err != nil {
		return false, fmt.Errorf("store: migration: look for %s: %w", name, err)
	}
	return n > 0, nil
}

// plainDevices returns the whatsmeow_device rows with a non-zero private column.
func plainDevices(ctx context.Context, conn *sql.Conn) ([]plainDevice, error) {
	if ok, err := tableExists(ctx, conn, "whatsmeow_device"); err != nil || !ok {
		return nil, err
	}
	rows, err := conn.QueryContext(ctx, `SELECT jid, noise_key, identity_key, signed_pre_key, adv_key FROM whatsmeow_device ORDER BY jid`)
	if err != nil {
		return nil, fmt.Errorf("store: migration: read whatsmeow_device: %w", err)
	}
	defer rows.Close()
	var out []plainDevice
	for rows.Next() {
		var d plainDevice
		if err := rows.Scan(&d.jid, &d.noise, &d.identity, &d.signedPreKey, &d.advSecret); err != nil {
			return nil, fmt.Errorf("store: migration: read whatsmeow_device: %w", err)
		}
		if isZero(d.noise) && isZero(d.identity) && isZero(d.signedPreKey) && isZero(d.advSecret) {
			continue
		}
		out = append(out, d)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("store: migration: read whatsmeow_device: %w", err)
	}
	return out, nil
}

func isZero(b []byte) bool { return len(bytes.Trim(b, "\x00")) == 0 }

// hasSealCheck reports whether nuncio_seal_meta holds its row.
func hasSealCheck(ctx context.Context, conn *sql.Conn) (bool, error) {
	if ok, err := tableExists(ctx, conn, "nuncio_seal_meta"); err != nil || !ok {
		return false, err
	}
	var n int
	if err := conn.QueryRowContext(ctx, `SELECT COUNT(*) FROM nuncio_seal_meta WHERE id = 1`).Scan(&n); err != nil {
		return false, fmt.Errorf("store: migration: read nuncio_seal_meta: %w", err)
	}
	return n > 0, nil
}

// writeBackup copies the database with VACUUM INTO to <base>.bak, or, when a file has that name,
// to the first free <base>-<n>.bak up to maxBackupNumber, and returns the path. The file is
// created empty, 0600 and exclusively first, so no byte lands in a file anyone else can read and
// an existing file is never overwritten. Errors name the file by its base name only: the path
// names the user's home.
func writeBackup(ctx context.Context, conn *sql.Conn, base string) (string, error) {
	var path string
	var f *os.File
	for n := 0; n <= maxBackupNumber; n++ {
		path = base + ".bak"
		if n > 0 {
			path = fmt.Sprintf("%s-%d.bak", base, n)
		}
		var err error
		f, err = os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
		if errors.Is(err, fs.ErrExist) {
			continue
		}
		if err != nil {
			return "", fmt.Errorf("store: migration: create the backup %s: %w", filepath.Base(path), unwrapPath(err))
		}
		break
	}
	if f == nil {
		return "", fmt.Errorf("%w: %s.bak and its numbered names", ErrBackupExists, filepath.Base(base))
	}
	name := filepath.Base(path)
	if err := f.Chmod(0o600); err != nil {
		_ = f.Close()
		return "", fmt.Errorf("store: migration: make the backup %s private: %w", name, unwrapPath(err))
	}
	if err := f.Close(); err != nil {
		return "", fmt.Errorf("store: migration: create the backup %s: %w", name, unwrapPath(err))
	}
	if _, err := conn.ExecContext(ctx, `VACUUM INTO ?`, path); err != nil {
		return "", fmt.Errorf("store: migration: write the backup %s: %w", name, err)
	}
	return path, nil
}

// unwrapPath drops the path from an *fs.PathError, keeping its operation's error.
func unwrapPath(err error) error {
	var pe *fs.PathError
	if errors.As(err, &pe) {
		return pe.Err
	}
	return err
}

// verifyBackup opens the backup read-only and checks that it is a sound database holding every
// device row the migration is about to seal.
func verifyBackup(ctx context.Context, path string, devices []plainDevice) error {
	name := filepath.Base(path)
	u := url.URL{Scheme: "file", Path: path, RawQuery: "mode=ro"}
	db, err := sql.Open(driverName, u.String())
	if err != nil {
		return fmt.Errorf("store: migration: open the backup %s: %w", name, err)
	}
	defer db.Close()
	db.SetMaxOpenConns(1)
	var check string
	if err := db.QueryRowContext(ctx, `PRAGMA integrity_check`).Scan(&check); err != nil {
		return fmt.Errorf("store: migration: check the backup %s: %w", name, err)
	}
	if check != "ok" {
		return fmt.Errorf("store: migration: the backup %s fails its integrity check", name)
	}
	for _, d := range devices {
		var noise []byte
		err := db.QueryRowContext(ctx, `SELECT noise_key FROM whatsmeow_device WHERE jid = ?`, d.jid).Scan(&noise)
		if err != nil || !bytes.Equal(noise, d.noise) {
			clear(noise)
			return fmt.Errorf("store: migration: the backup %s does not hold the device", name)
		}
		clear(noise)
	}
	return nil
}

// sealInPlace runs the one migration transaction and returns the rows per table and the sealer
// of the data key it sealed under. It takes SQLite's write lock first (BEGIN IMMEDIATE) and only
// then looks for the plain device keys again and takes or creates the data key, so no other
// migration can create a key or commit between its look and its commit. With no plain device
// keys left it is errMigratedMeanwhile, and nothing was written, the keyring included.
func sealInPlace(ctx context.Context, conn *sql.Conn, kr Keyring) (map[string]int, *Sealer, error) {
	// Only the write lock waits for another process: the cleanup after the commit fails fast on
	// a reader, as before, and is retried.
	restoreBusy, err := busyTimeoutOn(ctx, conn)
	if err != nil {
		return nil, nil, err
	}
	defer restoreBusy()
	if _, err := conn.ExecContext(ctx, `BEGIN IMMEDIATE`); err != nil {
		return nil, nil, fmt.Errorf("store: migration: take the write lock: %w", err)
	}
	committed := false
	defer func() {
		if committed {
			return
		}
		if _, err := conn.ExecContext(context.WithoutCancel(ctx), `ROLLBACK`); err != nil {
			// A connection that may still be inside the transaction never goes back to the pool.
			_ = conn.Raw(func(any) error { return driver.ErrBadConn })
		}
	}()
	tx := conn // the statements below run inside the transaction BEGIN opened on conn

	devices, err := plainDevices(ctx, tx)
	if err != nil {
		return nil, nil, err
	}
	defer func() {
		for _, d := range devices {
			clear(d.noise)
			clear(d.identity)
			clear(d.signedPreKey)
			clear(d.advSecret)
		}
	}()
	if len(devices) == 0 {
		return nil, nil, errMigratedMeanwhile
	}
	if sealedMeta, err := hasSealCheck(ctx, tx); err != nil {
		return nil, nil, err
	} else if sealedMeta {
		return nil, nil, ErrMigrationInconsistent
	}
	key, err := LoadDataKey(kr, true)
	if err != nil {
		return nil, nil, fmt.Errorf("store: migration: %w", err)
	}
	defer clear(key)
	sealer, err := NewSealer(key)
	if err != nil {
		return nil, nil, err
	}

	for _, schema := range []string{deviceSchema, preKeySchema, sealMetaSchema, cleanupMarkSchema} {
		if _, err := tx.ExecContext(ctx, schema); err != nil {
			return nil, nil, fmt.Errorf("store: migration: create the sealed tables: %w", err)
		}
	}
	// The cleanup-pending mark commits with the sealed values: no commit without it.
	if _, err := tx.ExecContext(ctx, `INSERT OR IGNORE INTO nuncio_seal_cleanup (id) VALUES (1)`); err != nil {
		return nil, nil, fmt.Errorf("store: migration: write the cleanup mark: %w", err)
	}
	rows := map[string]int{}
	for _, c := range migratedColumns {
		n, err := sealColumn(ctx, tx, sealer, c)
		if err != nil {
			return nil, nil, err
		}
		rows[c.table] = n
		if err := fault("sealed " + c.table); err != nil {
			return nil, nil, err
		}
	}
	n, err := movePreKeys(ctx, tx, sealer)
	if err != nil {
		return nil, nil, err
	}
	rows["whatsmeow_pre_keys"] = n
	if err := fault("moved whatsmeow_pre_keys"); err != nil {
		return nil, nil, err
	}
	if err := sealDevices(ctx, tx, sealer, devices); err != nil {
		return nil, nil, err
	}
	rows["whatsmeow_device"] = len(devices)
	if err := fault("sealed whatsmeow_device"); err != nil {
		return nil, nil, err
	}

	// WriteSealCheck's row, written inside this transaction so the check and the sealed values
	// commit together.
	if _, err := tx.ExecContext(ctx, `INSERT INTO nuncio_seal_meta (id, version, "check") VALUES (1, ?, ?)`,
		sealVersion, sealer.Seal(sealCheckAAD(), []byte(sealCheckPlaintext))); err != nil {
		return nil, nil, fmt.Errorf("store: migration: write the seal check row: %w", err)
	}
	if err := fault("check"); err != nil {
		return nil, nil, err
	}
	// L5 A1: a writer outside the migration (a link's first save) may have replaced the key since
	// LoadDataKey; commit only while the keyring still holds the key everything here is sealed
	// under.
	back, err := readDataKey(kr)
	if err != nil {
		return nil, nil, fmt.Errorf("store: migration: read the data key again: %w", err)
	}
	same := bytes.Equal(back, key)
	clear(back)
	if !same {
		return nil, nil, fmt.Errorf("%w: %s changed during the migration (service %s)", ErrKeyringUnavailable, itemDataKey, KeyringService)
	}
	if _, err := tx.ExecContext(ctx, `COMMIT`); err != nil {
		return nil, nil, fmt.Errorf("store: migration: commit: %w", err)
	}
	committed = true
	return rows, sealer, nil
}

// sealColumn seals every value of one column in place, bound like sealedStore binds it. A NULL
// stays NULL, as sealedStore stores a nil value.
func sealColumn(ctx context.Context, tx *sql.Conn, sealer *Sealer, c migratedColumn) (int, error) {
	cols := fmt.Sprintf("rowid, %q, %q", c.ourCol, c.column)
	for _, p := range c.pk {
		cols += fmt.Sprintf(", %q", p)
	}
	type row struct {
		rowid int64
		vals  [][]byte // our JID, value, primary key parts
	}
	var all []row
	q, err := tx.QueryContext(ctx, fmt.Sprintf(`SELECT %s FROM %q`, cols, c.table))
	if err != nil {
		return 0, fmt.Errorf("store: migration: read %s: %w", c.table, err)
	}
	for q.Next() {
		r := row{vals: make([][]byte, 2+len(c.pk))}
		dest := []any{&r.rowid}
		for i := range r.vals {
			dest = append(dest, &r.vals[i])
		}
		if err := q.Scan(dest...); err != nil {
			_ = q.Close()
			return 0, fmt.Errorf("store: migration: read %s: %w", c.table, err)
		}
		all = append(all, r)
	}
	if err := q.Err(); err != nil {
		_ = q.Close()
		return 0, fmt.Errorf("store: migration: read %s: %w", c.table, err)
	}
	_ = q.Close()

	update := fmt.Sprintf(`UPDATE %q SET %q = ? WHERE rowid = ?`, c.table, c.column)
	for _, r := range all {
		value := r.vals[1]
		if value == nil {
			continue
		}
		pk := make([]string, len(c.pk))
		for i := range c.pk {
			pk[i] = string(r.vals[2+i])
		}
		sealed := sealer.Seal(sealAAD(c.table, c.column, string(r.vals[0]), pk...), value)
		clear(value)
		if _, err := tx.ExecContext(ctx, update, sealed, r.rowid); err != nil {
			return 0, fmt.Errorf("store: migration: seal %s: %w", c.table, err)
		}
	}
	return len(all), nil
}

// movePreKeys moves every pre-key from whatsmeow_pre_keys into nuncio_pre_keys, sealed like
// sealedStore seals a pre-key it generates.
func movePreKeys(ctx context.Context, tx *sql.Conn, sealer *Sealer) (int, error) {
	type preKey struct {
		jid      string
		id       uint32
		key      []byte
		uploaded bool
	}
	var all []preKey
	q, err := tx.QueryContext(ctx, `SELECT jid, key_id, key, uploaded FROM whatsmeow_pre_keys`)
	if err != nil {
		return 0, fmt.Errorf("store: migration: read whatsmeow_pre_keys: %w", err)
	}
	for q.Next() {
		var k preKey
		if err := q.Scan(&k.jid, &k.id, &k.key, &k.uploaded); err != nil {
			_ = q.Close()
			return 0, fmt.Errorf("store: migration: read whatsmeow_pre_keys: %w", err)
		}
		all = append(all, k)
	}
	if err := q.Err(); err != nil {
		_ = q.Close()
		return 0, fmt.Errorf("store: migration: read whatsmeow_pre_keys: %w", err)
	}
	_ = q.Close()
	for _, k := range all {
		sealed := sealer.Seal(sealAAD(tPreKeys, "sealed", k.jid, preKeyAAD(k.id)), k.key)
		clear(k.key)
		if _, err := tx.ExecContext(ctx, `INSERT INTO nuncio_pre_keys (jid, key_id, sealed, uploaded) VALUES (?, ?, ?, ?)`,
			k.jid, k.id, sealed, k.uploaded); err != nil {
			return 0, fmt.Errorf("store: migration: move a pre-key: %w", err)
		}
	}
	if _, err := tx.ExecContext(ctx, `DELETE FROM whatsmeow_pre_keys`); err != nil {
		return 0, fmt.Errorf("store: migration: empty whatsmeow_pre_keys: %w", err)
	}
	return len(all), nil
}

// sealDevices seals each device's keys into nuncio_device, like PutDevice, and leaves the zeroed
// stub row in whatsmeow_device.
func sealDevices(ctx context.Context, tx *sql.Conn, sealer *Sealer, devices []plainDevice) error {
	for _, d := range devices {
		if len(d.noise) != devicePrivSize || len(d.identity) != devicePrivSize || len(d.signedPreKey) != devicePrivSize || len(d.advSecret) == 0 {
			return errors.New("store: migration: a whatsmeow_device row has keys of the wrong length")
		}
		sealed := sealDeviceKeys(sealer, d.jid, d.noise, d.identity, d.signedPreKey, d.advSecret)
		if _, err := tx.ExecContext(ctx, `INSERT INTO nuncio_device (jid, sealed) VALUES (?, ?)`, d.jid, sealed); err != nil {
			return fmt.Errorf("store: migration: write nuncio_device: %w", err)
		}
		if _, err := tx.ExecContext(ctx, `UPDATE whatsmeow_device SET noise_key = ?, identity_key = ?, signed_pre_key = ?, adv_key = ? WHERE jid = ?`,
			zeroKey, zeroKey, zeroKey, zeroKey, d.jid); err != nil {
			return fmt.Errorf("store: migration: zero the whatsmeow_device keys: %w", err)
		}
	}
	return nil
}

// wipeFreedPages overwrites what the migration freed: with secure_delete on, the WAL is
// checkpointed into the database and truncated, VACUUM rebuilds the database without free pages,
// and a last checkpoint moves the rebuilt pages over the old ones and truncates the WAL again.
func wipeFreedPages(ctx context.Context, conn *sql.Conn) error {
	checkpoint := func() error {
		var busy, logPages, done int
		if err := conn.QueryRowContext(ctx, `PRAGMA wal_checkpoint(TRUNCATE)`).Scan(&busy, &logPages, &done); err != nil {
			return fmt.Errorf("store: migration: checkpoint the WAL: %w", err)
		}
		if busy != 0 {
			return errors.New("store: migration: the WAL checkpoint was blocked by another reader")
		}
		return nil
	}
	if err := checkpoint(); err != nil {
		return err
	}
	if err := fault("checkpointed"); err != nil {
		return err
	}
	if _, err := conn.ExecContext(ctx, `VACUUM`); err != nil {
		return fmt.Errorf("store: migration: vacuum: %w", err)
	}
	return checkpoint()
}

// verifySealCheckConn is VerifySealCheck on a held connection.
func verifySealCheckConn(ctx context.Context, conn *sql.Conn, s *Sealer) error {
	var version int64
	var check []byte
	if err := conn.QueryRowContext(ctx, `SELECT version, "check" FROM nuncio_seal_meta WHERE id = 1`).Scan(&version, &check); err != nil {
		return fmt.Errorf("store: read the seal check row: %w", err)
	}
	plaintext, err := s.Open(sealCheckAAD(), check)
	if err != nil || version != sealVersion || string(plaintext) != sealCheckPlaintext {
		return ErrWrongDataKey
	}
	return nil
}
