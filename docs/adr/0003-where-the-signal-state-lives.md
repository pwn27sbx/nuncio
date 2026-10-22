# ADR 0003: where the Signal state lives

- Status: accepted (T17, Q5 option 1); planned until the home acceptance run
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
   bytes from `crypto/rand`. It is created once, then read back and compared before anything is
   sealed under it, so a keyring that drops a write is caught before data depends on it.
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
   - pre-keys live in nuncio's own table (`nuncio_pre_keys`), generated and sealed by the wrapper;
   - the PN-to-LID migration is overridden: each moved row is opened under its old address and
     sealed again under the new one;
   - the device's private keys are kept by nuncio's device store, sealed under the data key, not as
     one keyring item per key.
7. **What is sealed and what stays plain** (S6):

   | Sealed | Plain |
   | --- | --- |
   | `whatsmeow_device` keys (`noise_key`, `identity_key`, `signed_pre_key`, `adv_key`; see 8) | JIDs, ids, timestamps, flags |
   | pre-keys | contacts' public identity keys |
   | `sessions.session` | signatures and public device fields |
   | `sender_keys.sender_key` | app-state version hashes and MACs |
   | `app_state_sync_keys.key_data` | contacts, chat settings |
   | `message_secrets.key` | `lid_map` |
   | `privacy_tokens.token`, `nct_salt.salt` | nuncio's own tables (messages, chats, outbox, search) |
   | `event_buffer.plaintext`, `retry_buffer.plaintext` | |

8. **`whatsmeow_device` keeps a stub row.** Every Signal table has a foreign key to it, so the row
   with our JID must exist; its key columns hold zeroed bytes of the checked length, and the real
   keys are read from the sealed device record.
9. **A key check row.** One row in a nuncio table holds a known value sealed under the data key. At
   start it tells apart "no data yet", "the right key" and "a different or missing key" before
   anything else is read or written.
10. **The whatsmeow upgrade guard.** A test pins the method sets of the whatsmeow store interfaces
    the wrapper implements. A whatsmeow bump that adds or changes a method fails that test, so a new
    method cannot fall through to `SQLStore` and write key bytes in plain.

### Failure modes

11. **The keyring is locked or unavailable at start** (to confirm, L3 (3)): the daemon stays up and
    reports it to the clients, never connects to WhatsApp and never writes to the store; it retries
    the keyring on the session's capped backoff.
12. **The data key is missing but sealed data exists** (the key check row fails, or the item is
    gone): nuncio never creates a new key over sealed data. It says the device cannot be opened and
    tells the user to run `nuncio purge --confirm` and link again.
13. **A tampered or corrupted row:** GCM refuses it, and that one read fails with an error; there is
    no fallback to reading it as plain.

### Migrating an existing plain database

14. **Backup first.** Before touching a plain `nuncio.db`, the migration writes a copy with
    `VACUUM INTO` next to it (`0600`). It then seals the private columns in place in one
    transaction, and vacuums so no freed page keeps a plain key. It is idempotent (already sealed
    values are left alone), and a failure at any step leaves the original database as it was.
15. **The backup is never deleted by the migration** (to confirm, L3 (2)). It holds the plain keys,
    and nuncio says so when it writes it, so the user can remove it once the sealed database works.
16. **The wipe deletes the key and the backup.** `nuncio logout --confirm` and
    `nuncio purge --confirm` delete the `store.data_key` item with the other keyring items under
    `nuncio`, and the migration's backup with `nuncio.db`, so a wipe leaves nothing behind.

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
