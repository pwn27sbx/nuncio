package store

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"strings"
	"sync"
	"testing"
)

// countingKeyring counts the writes that reach a fake keyring.
type countingKeyring struct {
	*fakeKeyring
	mu   sync.Mutex
	sets int
}

func (c *countingKeyring) Set(service, user, secret string) error {
	c.mu.Lock()
	c.sets++
	c.mu.Unlock()
	return c.fakeKeyring.Set(service, user, secret)
}

func (c *countingKeyring) setCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.sets
}

// lyingKeyring reports a successful write but stores different bytes, like a
// keyring that silently mangles what it is given.
type lyingKeyring struct{ *fakeKeyring }

func (l lyingKeyring) Set(service, user, _ string) error {
	return l.fakeKeyring.Set(service, user, base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{7}, 32)))
}

func testKey(b byte) []byte { return bytes.Repeat([]byte{b}, 32) }

func newTestSealer(t *testing.T, key []byte) *Sealer {
	t.Helper()
	s, err := NewSealer(key)
	if err != nil {
		t.Fatalf("NewSealer: %v", err)
	}
	return s
}

// assertNoSecretIn fails when an error message carries the key or the
// plaintext, raw, hex or base64.
func assertNoSecretIn(t *testing.T, err error, secrets ...[]byte) {
	t.Helper()
	if err == nil {
		return
	}
	msg := err.Error()
	for _, s := range secrets {
		for _, form := range []string{string(s), hex.EncodeToString(s), base64.StdEncoding.EncodeToString(s)} {
			if form != "" && strings.Contains(msg, form) {
				t.Errorf("error %q carries secret bytes", msg)
			}
		}
	}
}

func TestSealerRoundTrip(t *testing.T) {
	s := newTestSealer(t, testKey(1))
	aad := sealAAD("sessions", "session", "15550000001@s.whatsapp.net", "15550000002.0")
	plaintext := []byte("a private signal session")

	sealed := s.Seal(aad, plaintext)

	if len(sealed) != 1+12+len(plaintext)+16 {
		t.Fatalf("sealed length = %d, want %d", len(sealed), 1+12+len(plaintext)+16)
	}
	if sealed[0] != 0x01 {
		t.Fatalf("version byte = %#x, want 0x01", sealed[0])
	}
	if bytes.Contains(sealed, plaintext) {
		t.Fatal("sealed value contains the plaintext")
	}
	got, err := s.Open(aad, sealed)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if !bytes.Equal(got, plaintext) {
		t.Fatalf("Open = %q, want %q", got, plaintext)
	}
}

func TestSealerRoundTripEmptyPlaintext(t *testing.T) {
	s := newTestSealer(t, testKey(1))
	aad := sealAAD("t", "c", "")
	got, err := s.Open(aad, s.Seal(aad, nil))
	if err != nil || len(got) != 0 {
		t.Fatalf("Open(Seal(empty)) = %q, %v; want empty, nil", got, err)
	}
}

func TestSealerUsesAFreshNonceEverySeal(t *testing.T) {
	s := newTestSealer(t, testKey(1))
	aad := sealAAD("t", "c", "")
	a := s.Seal(aad, []byte("same"))
	b := s.Seal(aad, []byte("same"))
	if bytes.Equal(a, b) {
		t.Fatal("two seals of the same plaintext are identical")
	}
	if bytes.Equal(a[1:13], b[1:13]) {
		t.Fatal("two seals reuse the same nonce")
	}
}

func TestSealerOpenRejects(t *testing.T) {
	key := testKey(1)
	s := newTestSealer(t, key)
	aad := sealAAD("sessions", "session", "15550000001@s.whatsapp.net", "x")
	plaintext := []byte("a private signal session")
	sealed := s.Seal(aad, plaintext)

	flip := func(i int) []byte {
		c := bytes.Clone(sealed)
		c[i] ^= 0x01
		return c
	}
	cases := []struct {
		name   string
		sealer *Sealer
		aad    []byte
		sealed []byte
	}{
		{"wrong version byte", s, aad, append([]byte{0x02}, sealed[1:]...)},
		{"zero version byte", s, aad, append([]byte{0x00}, sealed[1:]...)},
		{"empty input", s, aad, nil},
		{"version byte only", s, aad, sealed[:1]},
		{"shorter than nonce and tag", s, aad, sealed[:1+12+15]},
		{"wrong key", newTestSealer(t, testKey(2)), aad, sealed},
		{"wrong aad", s, sealAAD("sessions", "session", "15550000001@s.whatsapp.net", "y"), sealed},
		{"flipped nonce bit", s, aad, flip(1)},
		{"flipped ciphertext bit", s, aad, flip(13)},
		{"flipped tag bit", s, aad, flip(len(sealed) - 1)},
		{"truncated tag", s, aad, sealed[:len(sealed)-1]},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, err := tc.sealer.Open(tc.aad, tc.sealed)
			if !errors.Is(err, ErrUnseal) {
				t.Fatalf("Open error = %v, want ErrUnseal", err)
			}
			if got != nil {
				t.Fatalf("Open returned %d bytes alongside an error", len(got))
			}
			assertNoSecretIn(t, err, key, plaintext)
		})
	}
}

func TestNewSealerRejectsAKeyThatIsNot32Bytes(t *testing.T) {
	for _, n := range []int{0, 16, 24, 31, 33} {
		if _, err := NewSealer(make([]byte, n)); err == nil {
			t.Errorf("NewSealer(%d bytes) succeeded, want an error", n)
		}
	}
}

func TestSealAADIsUnambiguous(t *testing.T) {
	pairs := []struct {
		name string
		a, b []byte
	}{
		{"table/column boundary", sealAAD("ab", "c", "j"), sealAAD("a", "bc", "j")},
		{"column/jid boundary", sealAAD("t", "ab", "c"), sealAAD("t", "a", "bc")},
		{"jid/pk boundary", sealAAD("t", "c", "ab", "c"), sealAAD("t", "c", "a", "bc")},
		{"pk boundary", sealAAD("t", "c", "j", "ab", "c"), sealAAD("t", "c", "j", "a", "bc")},
		{"trailing empty pk", sealAAD("t", "c", "j", "a"), sealAAD("t", "c", "j", "a", "")},
	}
	for _, p := range pairs {
		if bytes.Equal(p.a, p.b) {
			t.Errorf("%s: AADs collide: %x", p.name, p.a)
		}
	}
	if !bytes.Equal(sealAAD("t", "c", "j", "k"), sealAAD("t", "c", "j", "k")) {
		t.Error("sealAAD is not deterministic")
	}
	if !bytes.Contains(sealAAD("t", "c", "j"), []byte("nuncio/seal/v1")) {
		t.Error("sealAAD does not bind the nuncio/seal/v1 label")
	}

	s := newTestSealer(t, testKey(1))
	sealed := s.Seal(sealAAD("ab", "c", "j"), []byte("x"))
	if _, err := s.Open(sealAAD("a", "bc", "j"), sealed); !errors.Is(err, ErrUnseal) {
		t.Fatalf("Open under a shifted-boundary AAD = %v, want ErrUnseal", err)
	}
}

func TestLoadDataKeyCreatesOnceAndReadsBack(t *testing.T) {
	kr := &countingKeyring{fakeKeyring: newFakeKeyring()}

	key, err := LoadDataKey(kr, true)
	if err != nil {
		t.Fatalf("LoadDataKey(create): %v", err)
	}
	if len(key) != 32 {
		t.Fatalf("key length = %d, want 32", len(key))
	}
	if bytes.Equal(key, make([]byte, 32)) {
		t.Fatal("key is all zeros")
	}
	if kr.setCount() != 1 {
		t.Fatalf("keyring writes = %d, want 1", kr.setCount())
	}
	stored, err := kr.fakeKeyring.Get("nuncio", "store.data_key")
	if err != nil {
		t.Fatalf("the key is not under nuncio/store.data_key: %v", err)
	}
	if stored != base64.StdEncoding.EncodeToString(key) {
		t.Fatal("the keyring item is not the base64 of the returned key")
	}

	again, err := LoadDataKey(kr, true)
	if err != nil {
		t.Fatalf("second LoadDataKey: %v", err)
	}
	if !bytes.Equal(again, key) {
		t.Fatal("second LoadDataKey returned a different key")
	}
	if kr.setCount() != 1 {
		t.Fatalf("keyring writes after reuse = %d, want 1", kr.setCount())
	}
}

func TestLoadDataKeyCreatesADifferentKeyEachTime(t *testing.T) {
	a, err := LoadDataKey(newFakeKeyring(), true)
	if err != nil {
		t.Fatal(err)
	}
	b, err := LoadDataKey(newFakeKeyring(), true)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Equal(a, b) {
		t.Fatal("two fresh keyrings got the same data key")
	}
}

func TestLoadDataKeyReusesAnExistingKeyUntouched(t *testing.T) {
	kr := &countingKeyring{fakeKeyring: newFakeKeyring()}
	existing := testKey(9)
	encoded := base64.StdEncoding.EncodeToString(existing)
	_ = kr.fakeKeyring.Set("nuncio", "store.data_key", encoded)

	for _, create := range []bool{true, false} {
		key, err := LoadDataKey(kr, create)
		if err != nil {
			t.Fatalf("LoadDataKey(create=%v): %v", create, err)
		}
		if !bytes.Equal(key, existing) {
			t.Fatalf("LoadDataKey(create=%v) did not return the stored key", create)
		}
	}
	if kr.setCount() != 0 {
		t.Fatalf("keyring writes = %d, want 0", kr.setCount())
	}
	if got, _ := kr.fakeKeyring.Get("nuncio", "store.data_key"); got != encoded {
		t.Fatal("the stored key changed")
	}
}

func TestLoadDataKeyDetectsAKeyringThatStoresOtherBytes(t *testing.T) {
	key, err := LoadDataKey(lyingKeyring{newFakeKeyring()}, true)
	if !errors.Is(err, ErrKeyringUnavailable) {
		t.Fatalf("LoadDataKey error = %v, want ErrKeyringUnavailable", err)
	}
	if key != nil {
		t.Fatal("a key was returned although the readback differs")
	}
}

func TestLoadDataKeyMissingWithoutCreate(t *testing.T) {
	kr := &countingKeyring{fakeKeyring: newFakeKeyring()}
	key, err := LoadDataKey(kr, false)
	if !errors.Is(err, ErrDataKeyMissing) {
		t.Fatalf("LoadDataKey error = %v, want ErrDataKeyMissing", err)
	}
	if key != nil {
		t.Fatal("a key was returned for a missing item")
	}
	if kr.setCount() != 0 || kr.count() != 0 {
		t.Fatalf("writes = %d, items = %d; want nothing written", kr.setCount(), kr.count())
	}
}

func TestLoadDataKeyKeyringFailures(t *testing.T) {
	cause := errors.New("dbus: the name org.freedesktop.secrets was not provided")
	cases := []struct {
		name string
		kr   *fakeKeyring
	}{
		{"get fails", &fakeKeyring{items: map[string]string{}, getErr: cause}},
		{"set fails", &fakeKeyring{items: map[string]string{}, setErr: cause}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			key, err := LoadDataKey(tc.kr, true)
			if !errors.Is(err, ErrKeyringUnavailable) {
				t.Fatalf("error = %v, want ErrKeyringUnavailable", err)
			}
			if !errors.Is(err, cause) {
				t.Fatalf("error = %v, does not wrap the keyring's cause", err)
			}
			if key != nil {
				t.Fatal("a key was returned alongside an error")
			}
		})
	}
}

func TestLoadDataKeyRejectsAnInvalidStoredKeyWithoutReplacingIt(t *testing.T) {
	for _, stored := range []string{"not base64 !!", base64.StdEncoding.EncodeToString(testKey(3)[:16])} {
		kr := &countingKeyring{fakeKeyring: newFakeKeyring()}
		_ = kr.fakeKeyring.Set("nuncio", "store.data_key", stored)
		key, err := LoadDataKey(kr, true)
		if !errors.Is(err, ErrDataKeyInvalid) {
			t.Fatalf("stored %q: error = %v, want ErrDataKeyInvalid", stored, err)
		}
		if key != nil {
			t.Fatal("a key was returned for an invalid item")
		}
		if strings.Contains(err.Error(), stored) {
			t.Fatalf("error %q carries the stored item", err)
		}
		if kr.setCount() != 0 {
			t.Fatal("an invalid key was overwritten")
		}
	}
}

func sealMetaRow(t *testing.T, db *sql.DB) (int, []byte) {
	t.Helper()
	var version int
	var check []byte
	if err := db.QueryRow(`SELECT version, "check" FROM nuncio_seal_meta`).Scan(&version, &check); err != nil {
		t.Fatalf("read nuncio_seal_meta: %v", err)
	}
	return version, check
}

func TestSealCheckPassesWithTheRightKey(t *testing.T) {
	ctx := context.Background()
	db, _ := openDBIn(t, t.TempDir())
	s := newTestSealer(t, testKey(1))

	if err := WriteSealCheck(ctx, db, s); err != nil {
		t.Fatalf("WriteSealCheck: %v", err)
	}
	if err := VerifySealCheck(ctx, db, newTestSealer(t, testKey(1))); err != nil {
		t.Fatalf("VerifySealCheck with the same key: %v", err)
	}
	version, check := sealMetaRow(t, db)
	if version != 1 || len(check) == 0 || check[0] != 0x01 {
		t.Fatalf("row = (%d, %x), want version 1 and a sealed value", version, check)
	}
}

func TestSealCheckFailsWithAWrongKeyAndWritesNothing(t *testing.T) {
	ctx := context.Background()
	db, _ := openDBIn(t, t.TempDir())
	wrongKey := testKey(2)
	if err := WriteSealCheck(ctx, db, newTestSealer(t, testKey(1))); err != nil {
		t.Fatalf("WriteSealCheck: %v", err)
	}
	_, before := sealMetaRow(t, db)

	wrong := newTestSealer(t, wrongKey)
	err := VerifySealCheck(ctx, db, wrong)
	if !errors.Is(err, ErrWrongDataKey) {
		t.Fatalf("VerifySealCheck with another key = %v, want ErrWrongDataKey", err)
	}
	assertNoSecretIn(t, err, wrongKey)
	if err := WriteSealCheck(ctx, db, wrong); !errors.Is(err, ErrWrongDataKey) {
		t.Fatalf("WriteSealCheck over an existing row with another key = %v, want ErrWrongDataKey", err)
	}
	_, after := sealMetaRow(t, db)
	if !bytes.Equal(before, after) {
		t.Fatal("the check row changed under a wrong key")
	}
}

func TestSealCheckWriteIsIdempotent(t *testing.T) {
	ctx := context.Background()
	db, _ := openDBIn(t, t.TempDir())
	s := newTestSealer(t, testKey(1))
	if err := WriteSealCheck(ctx, db, s); err != nil {
		t.Fatal(err)
	}
	_, first := sealMetaRow(t, db)
	if err := WriteSealCheck(ctx, db, s); err != nil {
		t.Fatalf("second WriteSealCheck: %v", err)
	}
	var rows int
	if err := db.QueryRow(`SELECT COUNT(*) FROM nuncio_seal_meta`).Scan(&rows); err != nil {
		t.Fatal(err)
	}
	_, second := sealMetaRow(t, db)
	if rows != 1 || !bytes.Equal(first, second) {
		t.Fatalf("rows = %d, row rewritten = %v; want one untouched row", rows, !bytes.Equal(first, second))
	}
}

func TestSealCheckMissingRow(t *testing.T) {
	ctx := context.Background()
	s := newTestSealer(t, testKey(1))

	t.Run("no table", func(t *testing.T) {
		db, _ := openDBIn(t, t.TempDir())
		if err := VerifySealCheck(ctx, db, s); !errors.Is(err, ErrSealCheckMissing) {
			t.Fatalf("VerifySealCheck on an empty database = %v, want ErrSealCheckMissing", err)
		}
		var n int
		if err := db.QueryRow(`SELECT COUNT(*) FROM sqlite_master WHERE name = 'nuncio_seal_meta'`).Scan(&n); err != nil {
			t.Fatal(err)
		}
		if n != 0 {
			t.Fatal("VerifySealCheck created the table")
		}
	})
	t.Run("empty table", func(t *testing.T) {
		db, _ := openDBIn(t, t.TempDir())
		if err := WriteSealCheck(ctx, db, s); err != nil {
			t.Fatal(err)
		}
		mustExec(t, db, `DELETE FROM nuncio_seal_meta`)
		if err := VerifySealCheck(ctx, db, s); !errors.Is(err, ErrSealCheckMissing) {
			t.Fatalf("VerifySealCheck on an empty table = %v, want ErrSealCheckMissing", err)
		}
	})
}

func TestSealCheckRejectsAnUnknownVersion(t *testing.T) {
	ctx := context.Background()
	db, _ := openDBIn(t, t.TempDir())
	s := newTestSealer(t, testKey(1))
	if err := WriteSealCheck(ctx, db, s); err != nil {
		t.Fatal(err)
	}
	mustExec(t, db, `UPDATE nuncio_seal_meta SET version = 2`)
	err := VerifySealCheck(ctx, db, s)
	if err == nil {
		t.Fatal("VerifySealCheck accepted an unknown version")
	}
	if errors.Is(err, ErrWrongDataKey) || errors.Is(err, ErrSealCheckMissing) {
		t.Fatalf("unknown version reported as %v", err)
	}
}

// hidingKeyring answers its first Get as if the item were absent although it holds one: another
// process wrote its key right after this caller looked.
type hidingKeyring struct {
	*countingKeyring
	hidden bool
}

func (h *hidingKeyring) Get(service, user string) (string, error) {
	if !h.hidden {
		h.hidden = true
		return "", ErrKeyNotFound
	}
	return h.countingKeyring.Get(service, user)
}

// L9 B1: a key that appears between LoadDataKey's first read and its write is never replaced;
// LoadDataKey returns that key.
func TestLoadDataKeyNeverReplacesAKeyThatAppearedMeanwhile(t *testing.T) {
	kr := &hidingKeyring{countingKeyring: &countingKeyring{fakeKeyring: newFakeKeyring()}}
	existing := testKey(5)
	_ = kr.fakeKeyring.Set("nuncio", "store.data_key", base64.StdEncoding.EncodeToString(existing))

	key, err := LoadDataKey(kr, true)
	if err != nil {
		t.Fatalf("LoadDataKey(create): %v", err)
	}
	if !bytes.Equal(key, existing) {
		t.Error("LoadDataKey did not return the key that appeared meanwhile")
	}
	if kr.setCount() != 0 {
		t.Errorf("keyring writes = %d, want 0", kr.setCount())
	}
	if got, _ := kr.fakeKeyring.Get("nuncio", "store.data_key"); got != base64.StdEncoding.EncodeToString(existing) {
		t.Error("the key that appeared meanwhile was replaced")
	}
}
