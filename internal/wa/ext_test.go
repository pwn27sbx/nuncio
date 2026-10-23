package wa

import (
	"testing"

	"github.com/pwn27sbx/nuncio/internal/store"
)

// The cache file extension: a document's own file name extension when it is safe, else the common
// extension of its mime type, else the mime subtype when it is safe, else none.
func TestCacheExtensionFromFileNameMimeTableOrSubtype(t *testing.T) {
	for _, tc := range []struct{ kind, mime, fileName, want string }{
		{store.KindDocument, "text/plain", "notes.TXT", "TXT"},
		{store.KindDocument, "application/pdf", "menu.pdf", "pdf"},
		{store.KindDocument, "text/plain", "", "txt"},
		{store.KindDocument, "text/plain", "no extension", "txt"},
		{store.KindDocument, "text/plain", "evil.t/xt", "txt"},
		{store.KindDocument, "application/octet-stream", "report.verylongext", ""},
		{store.KindImage, "image/jpeg", "photo.png", "jpg"},
		{store.KindAudio, "audio/ogg; codecs=opus", "", "ogg"},
		{store.KindAudio, "audio/mpeg", "", "mp3"},
		{store.KindAudio, "audio/mp4", "", "m4a"},
		{store.KindVideo, "video/quicktime", "", "mov"},
		{store.KindVideo, "video/mp4", "", "mp4"},
		{store.KindDocument, "text/csv", "", "csv"},
		{store.KindDocument, "application/msword", "", "doc"},
		{store.KindDocument, "application/vnd.ms-excel", "", "xls"},
		{store.KindDocument, "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "", "docx"},
		{store.KindDocument, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "", "xlsx"},
		{store.KindDocument, "application/vnd.openxmlformats-officedocument.presentationml.presentation", "", "pptx"},
		{store.KindImage, "Image/JPEG", "", "jpg"},
		{store.KindImage, "image/webp", "", "webp"},
		{store.KindDocument, "application/x-weird+thing", "", ""},
	} {
		if got := extFor(store.Message{Kind: tc.kind, Mime: tc.mime, FileName: tc.fileName}); got != tc.want {
			t.Errorf("extFor(%s, %q, %q) = %q, want %q", tc.kind, tc.mime, tc.fileName, got, tc.want)
		}
	}
}

// A document's file name never puts an executable or launcher extension into the cached name that
// xdg-open receives: only document and media extensions are taken from it, else the mime decides.
func TestADocumentFileNameNeverGivesAnExecutableExtension(t *testing.T) {
	for _, tc := range []struct{ mime, fileName, want string }{
		{"application/octet-stream", "run.sh", ""},
		{"text/plain", "evil.desktop", "txt"},
		{"application/java-archive", "app.jar", ""},
		{"application/octet-stream", "tool.AppImage", ""},
		{"application/pdf", "invoice.exe", "pdf"},
		{"text/x-python", "x.py", ""},
		{"application/pdf", "menu.PDF", "PDF"},
		{"application/zip", "photos.zip", "zip"},
		{"application/octet-stream", "song.flac", "flac"},
	} {
		if got := extFor(store.Message{Kind: store.KindDocument, Mime: tc.mime, FileName: tc.fileName}); got != tc.want {
			t.Errorf("extFor(%q, %q) = %q, want %q", tc.mime, tc.fileName, got, tc.want)
		}
	}
}
