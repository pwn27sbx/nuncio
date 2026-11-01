package wa

import (
	"errors"
	"fmt"
	"io"
	"strings"

	"github.com/mdp/qrterminal/v3"
)

// RenderQR draws `payload` as a QR code in the terminal.
//
// This is the one piece of the linking path that is pure and offline, so it is the one piece M0
// can actually prove: it renders a payload shaped like the one WhatsApp sends, without ever
// obtaining a real one.
func RenderQR(w io.Writer, payload string) error {
	if payload == "" {
		return nil
	}
	qrterminal.GenerateHalfBlock(payload, qrterminal.L, w)
	return nil
}

// quietZone is the margin a QR code needs around its matrix to scan: 4 modules on every side, in
// the terminal's background. The wire carries the matrix only, so a client draws it.
const quietZone = 4

// RenderMatrix draws a link_qr matrix as terminal art: one text row per two module rows (an upper
// half block for the module above, a lower half block for the one below), dark modules as ink, and
// the quiet zone around it. It writes the art and nothing else, so `nuncio link --plain-qr` can
// hand it to a script and no pairing payload ever travels as text.
//
// It refuses a matrix that does not match its own size: a code that cannot be drawn square is not
// the code the daemon says it is.
func RenderMatrix(w io.Writer, size int, rows []string) error {
	if size <= 0 {
		return fmt.Errorf("wa: a QR matrix of %d modules per side cannot be drawn", size)
	}
	if len(rows) != size {
		return fmt.Errorf("wa: a QR matrix of %d modules per side carries %d rows", size, len(rows))
	}
	for i, row := range rows {
		if len(row) != size {
			return fmt.Errorf("wa: row %d of a %d-module QR matrix is %d modules wide", i, size, len(row))
		}
		for _, module := range row {
			if module != '0' && module != '1' {
				return fmt.Errorf("wa: row %d of a %d-module QR matrix carries %q", i, size, module)
			}
		}
	}

	var out strings.Builder
	blank := strings.Repeat(" ", quietZone)
	for i := 0; i < quietZone; i++ {
		out.WriteString("\n")
	}
	for r := 0; r < size; r += 2 {
		out.WriteString(blank)
		for c := 0; c < size; c++ {
			upper := rows[r][c] == '1'
			lower := r+1 < size && rows[r+1][c] == '1'
			switch {
			case upper && lower:
				out.WriteString("█")
			case upper:
				out.WriteString("▀")
			case lower:
				out.WriteString("▄")
			default:
				out.WriteString(" ")
			}
		}
		out.WriteString(blank)
		out.WriteString("\n")
	}
	for i := 0; i < quietZone; i++ {
		out.WriteString("\n")
	}
	if _, err := io.WriteString(w, out.String()); err != nil {
		return errors.Join(errors.New("wa: cannot draw the QR code"), err)
	}
	return nil
}
