package main

import (
	"fmt"
	"io"
	"strings"
	"text/tabwriter"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// chatsCommand prints one line per chat: its name (or its masked number, see chatName), unread count,
// pinned/archived markers and the local time of its last message. Message text is never printed.
func chatsCommand(args []string, stdout, stderr io.Writer) int {
	if len(args) != 0 {
		fmt.Fprintf(stderr, "nuncio chats: takes no arguments\n\n%s\n", usage)
		return 2
	}
	c, err := ipc.Connect(ipc.ClientName)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio:", err)
		return 1
	}
	defer func() { _ = c.Close() }()
	resp, err := c.Chats(ipc.MaxChatLimit)
	if err != nil {
		fmt.Fprintln(stderr, "nuncio:", err)
		return 1
	}
	if len(resp.Chats) == 0 {
		fmt.Fprintln(stdout, "no chats")
		return 0
	}
	tw := tabwriter.NewWriter(stdout, 0, 0, 2, ' ', 0)
	for _, ch := range resp.Chats {
		fmt.Fprintf(tw, "%s\t%d unread\t%s\t%s\n", chatName(ch), ch.Unread, chatFlags(ch), lastTime(ch))
	}
	if err := tw.Flush(); err != nil {
		fmt.Fprintln(stderr, "nuncio:", err)
		return 1
	}
	return 0
}

// chatName is the display name, else the masked phone number the daemon serves, else the chat's
// own number masked to its last four digits. A LID chat (`digits@lid`) has no number of its own:
// its hidden id is never printed, masked or not.
func chatName(ch ipc.Chat) string {
	if ch.Name != "" {
		return ch.Name
	}
	if ch.Phone != "" {
		return ch.Phone
	}
	user, server, _ := strings.Cut(ch.JID, "@")
	if server == "lid" {
		return "hidden number"
	}
	return maskPhone(user)
}

func chatFlags(ch ipc.Chat) string {
	var flags []string
	if ch.Pinned {
		flags = append(flags, "pinned")
	}
	if ch.Archived {
		flags = append(flags, "archived")
	}
	if len(flags) == 0 {
		return "-"
	}
	return strings.Join(flags, ",")
}

func lastTime(ch ipc.Chat) string {
	if ch.LastMessage == nil {
		return "-"
	}
	t, err := time.Parse(ipc.TimestampFormat, ch.LastMessage.Timestamp)
	if err != nil {
		return "-"
	}
	return t.Local().Format("2006-01-02 15:04")
}
