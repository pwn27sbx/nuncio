package ipc

import (
	"bytes"
	"errors"
	"io"
	"strings"
	"testing"
)

func TestWriteFrameWritesOneNDJSONLine(t *testing.T) {
	var buf bytes.Buffer
	if err := WriteFrame(&buf, Hello{V: 1, Op: OpHello, Client: "nuncio-tui/0.1.0"}); err != nil {
		t.Fatalf("WriteFrame: %v", err)
	}
	// Exactly the shape of proto/fixtures/hello_client.json, plus the newline.
	const want = `{"v":1,"op":"hello","client":"nuncio-tui/0.1.0"}` + "\n"
	if got := buf.String(); got != want {
		t.Fatalf("frame = %q, want %q", got, want)
	}
}

func TestWriteFrameRefusesOversizedFrame(t *testing.T) {
	err := WriteFrame(io.Discard, Hello{V: 1, Op: OpHello, Client: strings.Repeat("a", MaxFrameLen)})
	if !errors.Is(err, ErrFrameTooLong) {
		t.Fatalf("err = %v, want ErrFrameTooLong", err)
	}
}

func TestReaderReadsFramesAndReportsEOF(t *testing.T) {
	r := NewReader(strings.NewReader(`{"v":1,"op":"status"}` + "\n" + `{"v":1,"op":"chats","limit":40}` + "\n"))

	var st StatusRequest
	if err := r.Read(&st); err != nil {
		t.Fatalf("Read status: %v", err)
	}
	if st.V != 1 || st.Op != OpStatus {
		t.Fatalf("status frame = %+v, want v=1 op=status", st)
	}

	var ch ChatsRequest
	if err := r.Read(&ch); err != nil {
		t.Fatalf("Read chats: %v", err)
	}
	if ch.V != 1 || ch.Op != OpChats || ch.Limit != 40 {
		t.Fatalf("chats frame = %+v, want v=1 op=chats limit=40", ch)
	}

	if err := r.Read(&st); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v, want io.EOF", err)
	}
}

// paddedHelloFrame returns a hello frame whose body is exactly n bytes.
func paddedHelloFrame(t *testing.T, n int) []byte {
	t.Helper()
	const overhead = len(`{"v":1,"op":"hello","client":""}`)
	if n < overhead {
		t.Fatalf("frame size %d is below the %d-byte envelope", n, overhead)
	}
	return []byte(`{"v":1,"op":"hello","client":"` + strings.Repeat("a", n-overhead) + `"}`)
}

func TestReaderAcceptsFrameAtSizeLimit(t *testing.T) {
	frame := paddedHelloFrame(t, MaxFrameLen)
	r := NewReader(bytes.NewReader(append(frame, '\n')))
	var h Hello
	if err := r.Read(&h); err != nil {
		t.Fatalf("Read at %d bytes: %v", MaxFrameLen, err)
	}
	if len(h.Client) == 0 || h.Op != OpHello {
		t.Fatalf("decoded frame = %+v, want a full hello frame", h)
	}
}

func TestReaderRejectsFrameOverSizeLimit(t *testing.T) {
	frame := paddedHelloFrame(t, MaxFrameLen+1)
	r := NewReader(bytes.NewReader(append(frame, '\n')))
	var h Hello
	if err := r.Read(&h); !errors.Is(err, ErrFrameTooLong) {
		t.Fatalf("err = %v, want ErrFrameTooLong", err)
	}
}

func TestReaderRejectsMalformedFrames(t *testing.T) {
	cases := []struct {
		name string
		line string
	}{
		{name: "not JSON", line: "{oops"},
		{name: "empty line", line: ""},
		{name: "JSON array", line: "[1,2,3]"},
		{name: "JSON string", line: `"hello"`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := NewReader(strings.NewReader(tc.line + "\n"))
			var h Hello
			if err := r.Read(&h); !errors.Is(err, ErrBadFrame) {
				t.Fatalf("err = %v, want ErrBadFrame", err)
			}
		})
	}
}
