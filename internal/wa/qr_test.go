package wa

import (
	"bytes"
	"strings"
	"testing"
	"unicode/utf8"
)

// The matrix the tests draw: 3 modules per side, so a test can compare the art exactly.
var testMatrix = []string{"101", "010", "111"}

func TestRenderMatrixDrawsTwoModuleRowsPerLineWithAFourModuleQuietZone(t *testing.T) {
	var out bytes.Buffer
	if err := RenderMatrix(&out, 3, testMatrix); err != nil {
		t.Fatalf("RenderMatrix: %v", err)
	}
	// Row 0 is 101 and row 1 is 010, so the first line is upper half, lower half, upper half;
	// row 2 is 111 and has no row below it, so its modules fill the upper half only.
	want := "\n\n\n\n" +
		"    ▀▄▀    \n" +
		"    ▀▀▀    \n" +
		"\n\n\n\n"
	if out.String() != want {
		t.Errorf("RenderMatrix wrote:\n%q\nwant:\n%q", out.String(), want)
	}
}

func TestRenderMatrixRefusesAMatrixThatDoesNotMatchItsSize(t *testing.T) {
	for _, tc := range []struct {
		name string
		size int
		rows []string
	}{
		{"no size", 0, nil},
		{"a row missing", 2, []string{"10"}},
		{"a row too long", 2, []string{"100", "01"}},
		{"a row too short", 2, []string{"10", "0"}},
		{"a character that is not a module", 2, []string{"10", "1x"}},
	} {
		var out bytes.Buffer
		if err := RenderMatrix(&out, tc.size, tc.rows); err == nil {
			t.Errorf("%s: RenderMatrix(size %d, %q) drew %q, want an error", tc.name, tc.size, tc.rows, out.String())
		}
	}
}

// The art is the only thing a `--plain-qr` reader gets: no heading, no ANSI, no trailing text.
func TestRenderMatrixWritesArtAndNothingElse(t *testing.T) {
	var out bytes.Buffer
	if err := RenderMatrix(&out, 3, testMatrix); err != nil {
		t.Fatal(err)
	}
	for _, line := range strings.Split(out.String(), "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		if strings.Trim(line, "▀▄█ ") != "" {
			t.Errorf("line %q carries something that is not art", line)
		}
		if strings.ContainsAny(line, "\x1b") {
			t.Errorf("line %q carries an escape sequence", line)
		}
	}
}

// A realistic code: the wire carries the matrix only, so the client adds the 4-module quiet zone on
// every side and one text row per two module rows.
func TestRenderMatrixDrawsARealisticCodeSquare(t *testing.T) {
	const size = 25
	rows := make([]string, size)
	for i := range rows {
		row := make([]byte, size)
		for j := range row {
			row[j] = '0'
			if (i+j)%2 == 0 {
				row[j] = '1'
			}
		}
		rows[i] = string(row)
	}

	var out bytes.Buffer
	if err := RenderMatrix(&out, size, rows); err != nil {
		t.Fatalf("RenderMatrix: %v", err)
	}
	var art []string
	for _, line := range strings.Split(strings.TrimSuffix(out.String(), "\n"), "\n") {
		if strings.TrimSpace(line) != "" {
			art = append(art, line)
		}
	}
	if want := (size + 1) / 2; len(art) != want {
		t.Errorf("%d art rows for %d module rows, want %d", len(art), size, want)
	}
	for i, line := range art {
		if want := 4 + size + 4; utf8.RuneCountInString(line) != want {
			t.Errorf("art row %d is %d wide, want %d (a 4-module quiet zone on each side)",
				i, utf8.RuneCountInString(line), want)
		}
	}
}
