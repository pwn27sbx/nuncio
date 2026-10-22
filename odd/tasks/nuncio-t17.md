# nuncio T17: Signal state behind a keyring-held data key (office, 2026-10-21)

Objective: no private key byte of the WhatsApp device or its Signal state is stored in plain text in any nuncio DB file; they are sealed under a data key held in the Secret Service keyring.
Branch: `feat/m4-omarchy`. Delivery: `single-pr` (work-unit commits). Runners: `TMPDIR=/tmp go test -race ./...`, `packaging/check.sh`.

## Specs

- S1 (owner, polish S4, verbatim): "Q5/T17 option 1, a keyring-held data key with sealed columns". Option 1 as offered (docs/handoff-2026-10-19-office.md:203): "keyring-held data key + sealed DB columns".
- S2 (acceptance, docs/handoff-2026-10-19-office.md:243-245): an ADR first; tests with a fake keyring; a real Secret Service run at home; pass means no key byte in any DB file, and with the keyring stopped the daemon refuses to persist.
- S3 (owner approval L19 in nuncio-polish.md, verbatim): "T17 may implement the nuncio.db migration; it backs up the database before migrating and never deletes the original; on the Mac only test databases created by tests are used, never ~/.local/share/nuncio or the real keyring; the first real migration runs at home."
- S4 (owner approval L17): dependencies only with a GPL-3.0-compatible licence verified in source and noted in CREDITS; a dependency change gets a verifier. Plan: no new dependency (stdlib AES-256-GCM, existing zalando/go-keyring).
- S5 Rules (nuncio-polish S2): tests first with RED; GGA per commit; independent verifier for high-risk units; never NUNCIO_KEYRING_TESTS on the Mac; no key bytes, ids, numbers or names in logs.
- S6 Sealed: whatsmeow_device noise_key, identity_key, signed_pre_key, adv_key; pre-keys; sessions.session; sender_keys.sender_key; app_state_sync_keys.key_data; message_secrets.key; privacy_tokens.token; nct_salt.salt; event_buffer.plaintext and retry_buffer.plaintext. Plain: JIDs, ids, timestamps, flags, contacts' public identity keys, signatures and public device fields, app-state version hashes and MACs, contacts, chat settings, lid_map, nuncio's own tables.

## Tasks

| ID | Specs | Route | Status | Commit |
|---|---|---|---|---|
| U1 | S1, S2: ADR 0003 and planned rows in SECURITY.md and README.md | delegated writer (parallel with U2; docs) | done: docs/adr/0003-where-the-signal-state-lives.md (decision, sealed/plain table, failure modes, migration, wipe, upgrade guard, L3 decisions to confirm); SECURITY.md and README.md say planned until the home run; structural readback (line widths, links) | this commit |
| U2 | S1, S5: data key get-or-create with readback, AES-256-GCM seal/open with versioned format and AAD, key check row | delegated writer (parallel with U1); Risk 2 → verifier | done: internal/store/seal.go (LoadDataKey, NewSealer/Seal/Open, sealAAD, nuncio_seal_meta with WriteSealCheck/VerifySealCheck); 17 tests, 15/16 mutants killed (1 equivalent); verifier PASS, 0 blockers (200k nonces unique, every AAD/bit/truncation tamper fails, 3.4M fuzz execs no panic, no secret bytes in errors) | this commit |
| U3 | S1, S6: sealedStore over whatsmeow's SQLStore, nuncio_pre_keys, PN→LID override, DeviceStore on the data key with a zeroed stub row, method-set guard | delegated writer after U2; Risk 1, 2 → verifier | open | |
| U4 | S3: migration of a plain DB (backup, seal in place, vacuum), idempotent, failure leaves the original | delegated writer after U3; Risk 1 → verifier | open | |
| U5 | S2, S5: wiring in wa/link/nunciod/logout, keyring-unavailable and key-missing states, wipe | delegated writer after U4; Risk 2, 3 → verifier | open | |
| U6 | S2, S3: home acceptance checklist in the handoff | parent | open | |

## Log

- L1 (owner, 2026-10-21, polish S4): "Q5/T17 option 1, a keyring-held data key with sealed columns".
- L2 Design mapped by a read-only planner (2026-10-21): sealing in a SQL driver wrapper is not workable (length CHECKs, MigratePNToLID copies rows in SQL, pre-keys generated inside SQLStore); wrap *sqlstore.SQLStore instead. internal/store/device.go already has a Keyring interface, SecretService over zalando/go-keyring v0.2.8 (MIT, in CREDITS) and fakes; no binary uses DeviceStore today. whatsmeow_device keeps a stub row because every Signal table has a foreign key to it.
- L3 Decisions taken under polish L16 (most conservative), to confirm with the owner: (1) seal the event and retry buffer payloads; (2) the migration never deletes its plain backup and tells the user it holds the keys, while logout/purge (the wipe) deletes it so a wipe leaves nothing behind; (3) keyring locked or absent at startup: the daemon stays up, reports it, never connects and never writes, retrying on the session backoff.
- L4 U1 left for U2/U3 to fill into the ADR: the key-check table name, the backup file name and the keyring encoding of the key.
- L5 U2 verifier advisories, to resolve in U5: A1 two concurrent LoadDataKey(create=true) on an empty keyring can both succeed with different keys (Set overwrites), so one caller may seal under a key the keyring no longer holds — U5 must create the key from one serialized place and re-read the keyring after WriteSealCheck succeeds, comparing with the key in memory; A2 a future v2 must put its version in the AAD; A4 VerifySealCheck reports a corrupt check row as ErrWrongDataKey and an unknown version without a sentinel; A5 Open sets no busy_timeout, so a concurrent WriteSealCheck from another *sql.DB fails with SQLITE_BUSY (safe). Names for the ADR (L4): table nuncio_seal_meta, keyring item store.data_key under service nuncio, base64 of 32 bytes.
