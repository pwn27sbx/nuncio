package main

import (
	_ "embed"
	"encoding/json"
	"fmt"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// chatsFixture is a copy of proto/fixtures/chats_response.json. The copy
// exists because //go:embed cannot reach outside the package directory and the
// daemon must work from any working directory.
// TestEmbeddedChatListMatchesContractFixture fails if it drifts from the
// contract.
//
//go:embed testdata/chats_response.json
var chatsFixture []byte

// fixtureChats returns the chat list the daemon answers with while no WhatsApp
// device is paired, so the clients have something real to render.
func fixtureChats() ([]ipc.Chat, error) {
	var resp ipc.ChatsResponse
	if err := json.Unmarshal(chatsFixture, &resp); err != nil {
		return nil, fmt.Errorf("parse the embedded chat fixture: %w", err)
	}
	return resp.Chats, nil
}
