package main

import (
	"fmt"
	"time"

	"github.com/pwn27sbx/nuncio/internal/ipc"
)

// chat is one invented chat: its list entry and its messages, oldest first.
type chat struct {
	info ipc.Chat
	msgs []ipc.Message
	// files holds what a sticker download serves, by message id: each sticker its own picture.
	files map[string][]byte
}

// seed builds the invented chats, timestamped relative to now so they look recent. Every number is
// a fake 555 one; nothing here came from WhatsApp.
func seed(now time.Time) []*chat {
	at := func(ago time.Duration) string { return ipc.Timestamp(now.Add(-ago)) }
	text := func(id, body string, fromMe bool, ago time.Duration) ipc.Message {
		return ipc.Message{ID: id, Kind: ipc.KindText, Text: body, FromMe: fromMe, Timestamp: at(ago)}
	}

	anaQuote := text("DEMOA0004", "Yes! Same venue as last time?", false, 50*time.Minute)
	anaQuote.ReplyTo = "DEMOA0003"
	anaLiked := text("DEMOA0005", "Same venue, 8 pm 🎉", true, 45*time.Minute)
	anaLiked.Reactions = []ipc.Reaction{{Emoji: "❤️", FromMe: false}}
	ana := &chat{
		info: ipc.Chat{JID: "15551230001@s.whatsapp.net", Name: "Ana (demo)", Pinned: true, Unread: 2},
		msgs: []ipc.Message{
			text("DEMOA0001", "Hi! Are we still on for Friday?", false, 3*time.Hour),
			text("DEMOA0002", "Of course 👍", true, 2*time.Hour),
			text("DEMOA0003", "Great, the whole family is coming 👨‍👩‍👧‍👦", true, time.Hour),
			anaQuote,
			anaLiked,
			text("DEMOA0006", "Perfect, see you there", false, 5*time.Minute),
			text("DEMOA0007", "Bring the 🏳️‍🌈 flag!", false, 4*time.Minute),
		},
	}

	photo := ipc.Message{
		ID: "DEMOG0004", Kind: ipc.KindImage, Text: "The view from the hotel", FromMe: false,
		Timestamp: at(30 * time.Minute), Media: &ipc.Media{Mime: "image/png", Size: 4096, FileName: ""},
		Thumb: photoThumb(), ThumbMime: "image/jpeg", Width: photoWidth, Height: photoHeight,
	}
	stickerFiles := map[string][]byte{}
	sticker := func(id string, ago time.Duration, thumb, file []byte, animated bool) ipc.Message {
		stickerFiles[id] = file
		m := ipc.Message{
			ID: id, Kind: ipc.KindSticker, Timestamp: at(ago), Width: 512, Height: 512, Animated: animated,
			Media: &ipc.Media{Mime: "image/webp", Size: int64(len(file))},
		}
		if thumb != nil {
			m.Thumb, m.ThumbMime = thumb, "image/png"
		}
		return m
	}
	group := &chat{
		info: ipc.Chat{JID: "120363000000000001@g.us", Name: "Travel group (demo)", Unread: 4},
		msgs: []ipc.Message{
			text("DEMOG0001", "明天见！我们九点出发。", false, 5*time.Hour),
			text("DEMOG0002", "مرحبا، كيف حالك؟", false, 4*time.Hour),
			text("DEMOG0003", "שלום לכולם", false, 3*time.Hour),
			photo,
			sticker("DEMOG0007", 25*time.Minute, stickerThumb(stickerYellow), stickerYellowWebP, false),
			sticker("DEMOG0008", 23*time.Minute, stickerThumb(stickerPink), stickerPinkWebP, true),
			sticker("DEMOG0009", 22*time.Minute, nil, stickerBlueWebP, false),
			text("DEMOG0005", "Wow 😮", true, 20*time.Minute),
			text("DEMOG0006", "東京タワー 🗼 は最高でした", false, 10*time.Minute),
		},
		files: stickerFiles,
	}

	long := &chat{info: ipc.Chat{JID: "15551230002@s.whatsapp.net", Name: "Long thread (demo)", Unread: 1}}
	const longCount = 240
	for i := 1; i <= longCount; i++ {
		long.msgs = append(long.msgs, text(fmt.Sprintf("DEMOL%04d", i),
			fmt.Sprintf("Message %d of %d in a long thread", i, longCount), i%3 == 0,
			time.Duration(longCount-i+1)*2*time.Minute+time.Hour))
	}

	doc := ipc.Message{
		ID: "DEMOM0002", Kind: ipc.KindDocument, Text: "Your invoice", FromMe: false,
		Timestamp: at(26 * time.Hour), Media: &ipc.Media{Mime: "text/plain", Size: 120, FileName: "invoice-demo.txt"},
	}
	muted := &chat{
		info: ipc.Chat{JID: "15551230003@s.whatsapp.net", Name: "Shop bot (demo)", Muted: true, Unread: 3},
		msgs: []ipc.Message{
			text("DEMOM0001", "Your order has shipped", false, 27*time.Hour),
			doc,
			text("DEMOM0003", "Rate us from 1 to 5 ⭐", false, 25*time.Hour),
		},
	}

	archived := &chat{
		info: ipc.Chat{JID: "15551230004@s.whatsapp.net", Name: "Old school friend (demo)", Archived: true},
		msgs: []ipc.Message{
			text("DEMOR0001", "Long time no see!", false, 30*24*time.Hour),
			text("DEMOR0002", "Indeed, let's catch up soon", true, 29*24*time.Hour),
		},
	}

	unnamed := &chat{
		info: ipc.Chat{JID: "15551230005@s.whatsapp.net"},
		msgs: []ipc.Message{text("DEMOU0001", "Is this the right number for the demo?", false, 2*24*time.Hour)},
	}

	return []*chat{ana, group, long, muted, archived, unnamed}
}
