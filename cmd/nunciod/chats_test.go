package main

import (
	_ "embed"
	"encoding/json"
	"fmt"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// chatsFixture is a copy of proto/fixtures/chats_response.json. The copy
// exists because //go:embed cannot reach outside the package directory; it is
// test material only — the daemon never serves it, and an unlinked daemon
// answers an empty chat list (F8). cmd/nuncio-demo has its own seed.
// TestEmbeddedChatListMatchesContractFixture fails if it drifts from the
// contract.
//
//go:embed testdata/chats_response.json
var chatsFixture []byte

// fixtureChats returns the chat list the contract fixture carries, which the
// tests compare the embedded copy against.
func fixtureChats() ([]ipc.Chat, error) {
	var resp ipc.ChatsResponse
	if err := json.Unmarshal(chatsFixture, &resp); err != nil {
		return nil, fmt.Errorf("parse the embedded chat fixture: %w", err)
	}
	return resp.Chats, nil
}
