# ADR 0003: where the Signal state lives

- Status: accepted (T17, Q5 option 1); wired into `nunciod` and `nuncio link` and tested against a
  fake keyring; with the real Secret Service planned until the home acceptance run
- Date: 2026-10-21

## Context

whatsmeow keeps the linked device's Signal state in its own tables inside `nuncio.db`: the device's
noise, identity, signed pre-key and ADV keys, the pre-keys, every session, the sender keys, the
app-state sync keys and more. The file is `0600` and its directory `0700`, but the key bytes are
plain: a copy of `~/.local/share/nuncio/` (a backup, a bug report, a stolen disk without disk
encryption) carries the whole device. `internal/store/device.go` already holds a keyring-backed
device store (one Secret Service item per device key, over `zalando/go-keyring`), but no binary
uses it, and it covers only the four device keys, not the Signal tables.

The owner was offered two options (handoff Q5):

1. **A keyring-held data key with sealed DB columns.** One random key in the Secret Service; the
   private columns are encrypted under it, row by row, inside the ordinary `nuncio.db`.
2. **An encrypted DB file decrypted into tmpfs.** The whole database is encrypted at rest and
   decrypted into memory (`/dev/shm` or `$XDG_RUNTIME_DIR`) while the daemon runs.

The owner chose: "Q5/T17 option 1, a keyring-held data key with sealed columns".

## Decision

### The data key and the seal

1. **One keyring item holds the data key**: `store.data_key` under the service `nuncio`, 32 random
   bytes from `crypto/rand`, stored as their standard base64. It is created once, then read back
   and compared before anything is sealed under it, so a keyring that drops a write is caught
   before data depends on it.
2. **AES-256-GCM from the Go standard library** (`crypto/aes`, `crypto/cipher`), no new dependency
   and no cgo. Every seal draws a fresh random 96-bit nonce; at the volume of one linked device
   (thousands of rows, not billions) random nonces stay far below GCM's collision bound.
3. **Format of a sealed value:** `0x01 | nonce (12 bytes) | ciphertext | tag (16 bytes)`. The
   leading byte is the format version; any other value is refused, so a later format can coexist.
4. **Associated data binds each value to its place:** `"nuncio/seal/v1"`, the table, the column,
   our own JID and the row's primary-key fields. A sealed value copied into another row, column or
   account fails to open instead of being accepted.

### Where the sealing happens

5. **A wrapper over whatsmeow's `*sqlstore.SQLStore`**, not a SQL driver wrapper. The wrapper
   implements whatsmeow's store interfaces, seals on the way in and opens on the way out, and
   leaves the SQL to whatsmeow. A driver wrapper (rewriting values under `database/sql`) was mapped
   and is not workable:
   - whatsmeow's tables carry length `CHECK`s on key columns (32 bytes), which a sealed value
     (61 bytes for a 32-byte key) breaks;
   - `MigratePNToLID` copies rows between addresses in SQL, so values move without passing through
     Go, and a value bound to its old primary key would no longer open;
   - pre-keys are generated and inserted inside `SQLStore` itself, so the driver would see only the
     final `INSERT` with no notion of which bytes are private.
6. **What the wrapper takes over from `SQLStore`:**
   - pre-keys live in nuncio's own table (`nuncio_pre_keys`: `jid`, `key_id`, `sealed`,
     `uploaded`, with a cascading foreign key to `whatsmeow_device`), generated and sealed by the
     wrapper;
   - the PN-to-LID migration is overridden: each moved row is opened under its old address and
     sealed again under the new one;
   - the device's private keys are kept by nuncio's device store, sealed under the data key in
     `nuncio_device` (`jid`, `sealed`, `companion_meta_nonce`; `sealed` holds the noise, identity
     and signed pre-key private keys and the ADV secret in one value), not as one keyring item per
     key;
   - the companion meta nonce WhatsApp issues (whatsmeow sends it back as the
     `Companion_User_Secret` header) is sealed on its own in `nuncio_device.companion_meta_nonce`,
     same key and format, bound to (`nuncio_device`, `companion_meta_nonce`, our JID), `NULL` when
     WhatsApp sent none; unlike the keys it is replaced on every save that changes it.
7. **What is sealed and what stays plain** (S6):

   | Sealed | Plain |
   | --- | --- |
   | `whatsmeow_device` keys (`noise_key`, `identity_key`, `signed_pre_key`, `adv_key`; see 8) | JIDs, ids, timestamps, flags |
   | `whatsmeow_device.companion_meta_nonce` (into `nuncio_device`; owner, T17 L11) | |
   | pre-keys | contacts' public identity keys |
   | `sessions.session` | signatures and public device fields |
   | `sender_keys.sender_key` | app-state version hashes and MACs |
   | `app_state_sync_keys.key_data` | contacts, chat settings |
   | `message_secrets.key` | `lid_map` |
   | `privacy_tokens.token`, `nct_salt.salt` | nuncio's own tables (messages, chats, outbox, search) |
   | `event_buffer.plaintext`, `retry_buffer.plaintext` | |

8. **`whatsmeow_device` keeps a stub row.** Every Signal table has a foreign key to it, so the row
   with our JID must exist; its key columns hold zeroed bytes of the checked length and its
   `companion_meta_nonce` is empty, and the real keys and nonce are read from `nuncio_device`. A
   stub row whose nonce is not empty is a database not migrated yet: the device is not loaded.
9. **A key check row.** One row in `nuncio_seal_meta` holds a known value sealed under the data
   key. At start it tells apart "no data yet", "the right key" and "a different or missing key"
   before anything else is read or written.
10. **The whatsmeow upgrade guard.** A test (`TestSQLStoreMethodSetIsPinned`) pins the exported
    method set of `*sqlstore.SQLStore`, names and signatures, which covers every whatsmeow store
    interface the wrapper implements. A whatsmeow bump that adds or changes a method fails that
    test, so a new method cannot fall through to `SQLStore` and write key bytes in plain
    (`docs/bump-drill.md`).

### Failure modes

11. **The keyring is locked or unavailable at start** (to confirm, L3 (3)): the daemon stays up and
    serves the socket, never connects to WhatsApp and never writes to the store (the keyring is
    read before whatsmeow's schema upgrade, the migration's backup or anything else); it retries
    the keyring on the session's capped backoff. Clients see the existing states, no new wire
    field: `status` and `hello_ok` say `session: offline` (`linked: true`, no `stop_reason`, which
    stays WhatsApp's own stops), and a request that needs the session (send, retry, react,
    mark_read, download_media, logout) answers an `internal` error with a fixed text naming the
    locked keyring. The daemon logs that same fixed line at each attempt. A database with no device
    needs no key: the daemon stays `unlinked` and never asks the keyring. With a device stored, the
    daemon serves the socket before it first asks the keyring, and makes that first attempt in the
    background: on Linux the Secret Service may wait on an unlock prompt with no timeout, and
    meanwhile `status` answers `offline` and a request answers a fixed text saying the device is
    still opening. One attempt runs at a time, so a retry never opens a second prompt while one is
    waiting, and stopping the daemon does not wait for it.
12. **The data key is missing but sealed data exists** (the key check row fails, or the item is
    gone): nuncio never creates a new key over sealed data. The device stays closed, the daemon
    answers `offline` as in 11 and never retries on its own, and the error text, logged once and
    answered to requests, is "the stored device cannot be read; run nuncio purge --confirm, then
    nuncio link"; `nuncio link` prints it and links nothing.
13. **A tampered or corrupted row:** GCM refuses it, and that one read fails with an error; there is
    no fallback to reading it as plain.

### Migrating an existing plain database

14. **Backup first.** The migration (`store.MigrateToSealed`) runs only on a plain `nuncio.db`:
    a `whatsmeow_device` row with real private keys and no `nuncio_seal_meta` row. A check row
    over plain device keys is an error, never resealed. A whatsmeow schema older than the tables
    it seals (a table or column missing) is refused with `store.ErrSchemaOutdated` before the
    backup and the key: the caller runs whatsmeow's schema upgrade first, never the migration.
    Before touching the database it writes a copy with `VACUUM INTO` next to it,
    `nuncio.db.pre-t17-<UTC time>.bak` (for example `nuncio.db.pre-t17-20261021T133045Z.bak`), or
    `…-1.bak` up to `…-99.bak` when that name is taken (a retry within the same second), created
    empty at `0600` first and never over an existing file, and checks the copy read-only
    (integrity check, device row present). Only then does it take SQLite's write lock
    (`BEGIN IMMEDIATE`), look for the plain device keys again under it, and ask the keyring for the
    data key (reused when one is there, an earlier failed run's included; otherwise created, and a
    key that appeared meanwhile is used, never replaced). One transaction seals the private
    columns in place, moves the pre-keys into `nuncio_pre_keys`, seals the device keys into
    `nuncio_device`, zeroes them in `whatsmeow_device`, writes the check row and sets the
    cleanup-pending mark (the one row of `nuncio_seal_cleanup`, no secret), the companion meta
    nonce moving with the device keys; it commits only if the
    keyring, read again, still holds the key it sealed under. Then, with `secure_delete` on, a
    WAL checkpoint, `VACUUM` and a last checkpoint wipe the free pages where stock whatsmeow left
    old plain keys, and only then is the mark cleared. A failure before the commit leaves the
    database's content as it was. A failure after it (a reader blocking the checkpoint, a
    cancelled context, a full disk) returns `Migrated` with `CleanupPending`: the free pages may
    still hold plain keys, and every later run that finds the mark retries the cleanup, even with
    nothing left to migrate, and reports `Cleaned` when it finishes. With nothing to migrate and
    no mark, a run writes nothing.
    **A database sealed before the nonce was** (the device keys in `nuncio_device`, a check row,
    the nonce still plain in the stub row; only test databases, since the first real migration
    runs after this change and covers the nonce itself): the same call upgrades it by the same
    rules. It takes the data key from the keyring only when the check row proves it, never
    creates one, and without it writes nothing (the seal check then reports the database, 12); a
    keyring that cannot be read stops it before the backup. Then a backup, verified to hold the
    nonce; under the write lock one transaction that seals the nonce into `nuncio_device` (adding
    the column to an older table), empties it in the stub row and sets the cleanup-pending mark,
    committed only while the keyring still holds the key; then the same free-page cleanup. A
    second run finds nothing to do.
    **Two migrations at once** (`nunciod` at start or on a retry, and `nuncio link`, on the same
    plain database): the Secret Service has no compare-and-swap, so two processes that both find no
    key could each write one, and the later write would replace the key the earlier one sealed
    under. Creating the key only under the write lock rules that out: the second process waits for
    the lock (up to 30 s) until the first has committed, then finds no plain device keys, writes no
    key and starts its checks again, as if it had started after the first. Its backup, written
    before it waited, stays on disk like any backup (15, 17). `nuncio link` also refuses before it
    opens the store while `nunciod` answers on `core.sock`.
15. **The backup is never deleted by the migration** (to confirm, L3 (2)). It holds the plain keys,
    and nuncio says so when it writes it, so the user can remove it once the sealed database works.
16. **Where the data key is created.** Only from `nuncio link`, and only when pairing saves a new
    device (whatsmeow's `Device.Save`): the key is generated in memory when the link starts, then
    on that first save written, read back, the check row written, the keyring read again and
    compared (a key another process wrote meanwhile fails the save), and only then the device row
    written. A link that never pairs leaves no key and no row. The daemon never creates a key,
    except through the migration of a database that holds no sealed data yet (14).
17. **The wipe deletes the key and the backup.** `nuncio logout --confirm` and
    `nuncio purge --confirm` delete the `store.data_key` item with the other keyring items under
    `nuncio` (the four `device.*` items of the older keyring-backed device store included), and
    every migration backup (`nuncio.db.pre-t17-*.bak`, numbered ones and a journal file left beside
    one included; a symlink is removed as a link) with `nuncio.db`, so a wipe leaves nothing behind.

### Decisions to confirm with the owner

Taken under the most conservative reading (polish L16), each still to be confirmed (L3):

1. Seal the event and retry buffer payloads (`event_buffer.plaintext`, `retry_buffer.plaintext`).
2. The migration never deletes its plain backup and tells the user it holds the keys; the wipe
   deletes it.
3. With the keyring locked or absent at start, the daemon stays up, reports it, never connects and
   never writes, retrying on the session backoff.

## Alternatives considered

- **Option 2, an encrypted file decrypted into tmpfs.** Seals messages too, but the whole database
  sits decrypted in memory-backed files readable by the same user for as long as the daemon runs,
  writes since the last re-encryption are lost on a crash or power cut, and SQLite's WAL makes a
  consistent re-encryption point hard to get. Rejected by the owner.
- **A SQL driver wrapper.** Seals without touching whatsmeow's interfaces, but breaks on its length
  `CHECK`s, its in-SQL row copies and its in-store pre-key generation (point 5). Rejected.
- **SQLCipher.** Encrypts the whole file, but needs cgo, and the daemon stays pure Go. Rejected.
- **One keyring item per secret** (the current `device.go`). Fine for four device keys, not for
  thousands of sessions, pre-keys and sender keys. Rejected for the Signal tables.

## Consequences

- No private key byte of the device or its Signal state is stored in plain in `nuncio.db`, its
  `-wal` or `-shm`. Messages, chats and the search index stay plain, as before (directory `0700`,
  files `0600`).
- The protection is against reading the file, not against code running as the user: once the
  keyring is unlocked, any process of the user can ask the Secret Service for the data key.
- Without the keyring the daemon does not run the session. Losing the key means relinking.
- A whatsmeow upgrade is a reviewed change: the method-set guard has to be updated on purpose.
- Acceptance (S2): tests with a fake keyring, never the real one on the Mac; then a real Secret
  Service run at home, where no key byte is found in any DB file and, with the keyring stopped, the
  daemon refuses to persist. Until that run this ADR describes planned work.
