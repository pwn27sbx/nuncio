package store

import (
	"errors"
	"fmt"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"syscall"

	"github.com/pwn27sbx/nuncio/internal/media"
)

// What nuncio keeps in its data directory ($XDG_DATA_HOME/nuncio).
const (
	// DBFile is the SQLite database: messages, chats and, today, whatsmeow's Signal state.
	DBFile = "nuncio.db"
	// MediaDir is the media cache (internal/media).
	MediaDir = "media"
	// ExportsDir holds chat exports.
	ExportsDir = "exports"
)

// owned is every entry of the data dir that Wipe removes. Anything else in the directory is not
// nuncio's and stays.
var owned = []string{DBFile, DBFile + "-wal", DBFile + "-shm", MediaDir, ExportsDir}

// removeDataDir removes the emptied data dir itself. Tests replace it to reproduce what a
// sandbox does to that last rmdir.
var removeDataDir = os.Remove

// Wipe deletes everything nuncio keeps on this machine for the device, in this order: the device
// keys in kr (service KeyringService), then the database with its WAL and SHM files, the media
// cache and the exports in dataDir, and finally dataDir itself when nothing else is left in it.
// An emptied dataDir that the system refuses to remove (EBUSY, EROFS, EPERM or EACCES, as under
// the packaged systemd unit's sandbox) stays and is not an error; a file of nuncio's that could
// not be deleted always is.
// Close the database before calling it. It first closes the media cache (media.Close): a
// download still running in this process then fails with media.ErrClosed and creates nothing.
//
// It is idempotent: what is already gone is not an error. It never follows a symlink: a symlink
// inside dataDir is removed as a link, and a dataDir that is itself a symlink is refused before
// anything is deleted.
//
// The keyring items are the ones DeviceStore.DeleteDevice removes. No binary writes them yet
// (whatsmeow's sqlstore keeps the Signal state inside nuncio.db until the keyring-backed device
// store is wired), so today this step finds nothing; it runs anyway so a wipe never leaves keys
// behind once it is. A failing keyring does not stop the files from being deleted: the error is
// returned after them.
func Wipe(dataDir string, kr Keyring) error {
	if fi, err := os.Lstat(dataDir); err == nil && fi.Mode()&fs.ModeSymlink != 0 {
		return fmt.Errorf("store: %s is a symlink; nothing was deleted", dataDir)
	} else if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("store: %w", err)
	}

	// From here no download in this process writes into the media cache, so none can recreate
	// it, or dataDir, once it is deleted.
	media.Close(filepath.Join(dataDir, MediaDir))

	var errs []error
	for _, name := range keyItems {
		switch err := kr.Delete(KeyringService, name); {
		case err == nil, errors.Is(err, ErrKeyNotFound):
		default:
			errs = append(errs, fmt.Errorf("store: delete %s from the keyring (service %s): %w", name, KeyringService, err))
		}
	}
	// os.RemoveAll removes a symlink itself, never what it points at, and does not follow
	// symlinks below a directory it removes.
	for _, name := range owned {
		if err := os.RemoveAll(filepath.Join(dataDir, name)); err != nil {
			errs = append(errs, fmt.Errorf("store: %w", err))
		}
	}
	if len(errs) == 0 {
		// Remove fails on a directory that still holds something; that is left alone.
		if err := removeDataDir(dataDir); err != nil && !errors.Is(err, fs.ErrNotExist) {
			if entries, rerr := os.ReadDir(dataDir); rerr != nil || len(entries) == 0 {
				if rerr == nil && sandboxKeeps(err) {
					// The packaged systemd unit bind-mounts dataDir onto itself inside a
					// read-only tree, so the emptied dir cannot be removed: the wipe is complete.
					log.Printf("store: %s is empty and stays (%v)", dataDir, err)
				} else {
					errs = append(errs, fmt.Errorf("store: %w", err))
				}
			}
		}
	}
	return errors.Join(errs...)
}

// sandboxKeeps reports whether err is how Linux refuses to remove a directory that is a mount
// point (EBUSY) or that sits in a read-only tree (EROFS), or whose parent is not writable (EPERM,
// EACCES).
func sandboxKeeps(err error) bool {
	return errors.Is(err, syscall.EBUSY) || errors.Is(err, syscall.EROFS) ||
		errors.Is(err, syscall.EPERM) || errors.Is(err, syscall.EACCES)
}
