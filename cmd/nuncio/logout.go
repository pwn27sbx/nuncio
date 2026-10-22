package main

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"net"
	"os"
	"path/filepath"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
	"github.com/pwn27sbx/nuncio/internal/store"
	"github.com/pwn27sbx/nuncio/internal/wa"
)

// localKeyring is the keyring the CLI uses itself: the data key `nuncio link` creates and reads,
// and the items a purge without a running daemon empties. Tests replace it so that no test can
// touch the owner's real keyring.
var localKeyring store.Keyring = store.SecretService{}

// answerTimeout bounds the handshake that decides whether a daemon is running.
const answerTimeout = 2 * time.Second

// logoutCommand is `nuncio logout --confirm`: the running daemon tells WhatsApp to unlink this
// device through its own session, then deletes the keyring items, the database, media and
// exports, removes its socket and exits. Without a running daemon it refuses: connecting here
// would open a second session of the device, which replaces the daemon's stream.
func logoutCommand(args []string, stdout, stderr io.Writer) int {
	return teardown("logout", true, args, stdout, stderr)
}

// purgeCommand is `nuncio purge --confirm`: the same teardown without the network step, for a
// device that is already gone. With a running daemon the daemon does it and exits; without one
// the CLI deletes the local data itself. WhatsApp is never contacted.
func purgeCommand(args []string, stdout, stderr io.Writer) int {
	return teardown("purge", false, args, stdout, stderr)
}

func teardown(name string, unlink bool, args []string, stdout, stderr io.Writer) int {
	flags := flag.NewFlagSet("nuncio "+name, flag.ContinueOnError)
	flags.SetOutput(stderr)
	confirm := flags.Bool("confirm", false, "you understand that this deletes everything and cannot be undone")
	if err := flags.Parse(args); err != nil {
		return 2
	}
	if flags.NArg() != 0 {
		fmt.Fprintf(stderr, "nuncio %s: takes no arguments, only --confirm\n", name)
		return 2
	}
	dbPath, err := wa.DBPath()
	if err != nil {
		fmt.Fprintf(stderr, "nuncio %s: %v\n", name, err)
		return 1
	}
	dataDir := filepath.Dir(dbPath)

	// The gate comes first: nothing above this line has a side effect, and nothing below it runs
	// without --confirm.
	if !*confirm {
		if unlink {
			fmt.Fprintln(stderr, "nuncio logout unlinks this computer from your WhatsApp account and deletes everything nuncio keeps here:")
		} else {
			fmt.Fprintln(stderr, "nuncio purge deletes everything nuncio keeps here, without contacting WhatsApp (for a device already removed from your phone's Linked devices):")
		}
		fmt.Fprintf(stderr, "  %s (with its -wal and -shm files)\n  %s (the migration's backups, which hold the keys in plain)\n  %s\n  %s\n  the nuncio items in the keyring (the data key included)\nand stops nunciod. Run it again with --confirm to do it. Nothing was deleted.\n",
			dbPath, dbPath+".pre-t17-*.bak", filepath.Join(dataDir, store.MediaDir), filepath.Join(dataDir, store.ExportsDir))
		return 2
	}

	socket, sockErr := ipc.DefaultSocketPath()
	var c *ipc.Client
	if sockErr == nil {
		c, err = ipc.DialTimeout(socket, ipc.ClientName, answerTimeout)
	}
	if sockErr == nil && err == nil {
		defer func() { _ = c.Close() }()
		return askDaemon(c, name, unlink, stdout, stderr)
	}

	if unlink {
		fmt.Fprintln(stderr, "nuncio logout: nunciod is not running. Only the daemon may talk to WhatsApp for this device (a second session would replace its stream): start nunciod and run this again, or, if the device is already gone from your phone's Linked devices, use nuncio purge --confirm. Nothing was deleted.")
		return 1
	}
	if sockErr == nil {
		// Something that accepts the connection but did not answer the handshake is a wedged
		// daemon: deleting the data under it would leave it running on nothing.
		if conn, err := net.DialTimeout("unix", socket, answerTimeout); err == nil {
			_ = conn.Close()
			fmt.Fprintf(stderr, "nuncio purge: something listens on %s but does not answer; stop nunciod first. Nothing was deleted.\n", socket)
			return 1
		}
	}
	if err := store.Wipe(dataDir, localKeyring); err != nil {
		fmt.Fprintln(stderr, "nuncio purge:", err)
		return 1
	}
	if sockErr == nil {
		// Nothing answers on it, so it is left over from a dead daemon.
		if err := os.Remove(socket); err != nil && !errors.Is(err, fs.ErrNotExist) {
			fmt.Fprintln(stderr, "nuncio purge: the data is deleted, but the stale socket stays:", err)
			return 1
		}
	}
	fmt.Fprintln(stdout, "nuncio: purged; the local data and keys are deleted. WhatsApp was not contacted.")
	return 0
}

// askDaemon has the running daemon do the teardown; the daemon exits once it answers.
func askDaemon(c *ipc.Client, name string, unlink bool, stdout, stderr io.Writer) int {
	_, err := c.Wipe(unlink)
	var perr *ipc.ProtocolError
	switch {
	case err == nil && unlink:
		fmt.Fprintln(stdout, "nuncio: logged out; WhatsApp unlinked this device, the local data and keys are deleted and nunciod stopped.")
		return 0
	case err == nil:
		fmt.Fprintln(stdout, "nuncio: purged; the local data and keys are deleted and nunciod stopped. WhatsApp was not contacted.")
		return 0
	case errors.As(err, &perr) && perr.Code == ipc.CodeNotLinked:
		fmt.Fprintln(stderr, "nuncio logout: no WhatsApp device is linked; to delete the local data use nuncio purge --confirm. Nothing was deleted.")
		return 1
	case errors.As(err, &perr):
		fmt.Fprintf(stderr, "nuncio %s: %s\n", name, perr.Message)
		return 1
	}
	fmt.Fprintf(stderr, "nuncio %s: %v\n", name, err)
	return 1
}
