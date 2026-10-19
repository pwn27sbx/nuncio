package ipc

import (
	"encoding/json"
	"log"
	"sync"
)

// SubscriberQueue is how many event frames the daemon holds for one subscriber that has not read
// them yet. A subscriber whose queue is full when another event is published is dropped: its
// queue is closed and its connection is closed, so a client that stops reading can never block
// the daemon or the other clients.
const SubscriberQueue = 64

// Broker fans event frames out to subscribers. It is safe for concurrent use, and a nil *Broker
// is a broker with no subscribers.
type Broker struct {
	mu   sync.Mutex
	subs map[*subscription]struct{}
}

type subscription struct {
	frames chan []byte
	onDrop func()
}

// NewBroker returns a broker with no subscribers.
func NewBroker() *Broker { return &Broker{subs: map[*subscription]struct{}{}} }

// Subscribe registers a subscriber. frames yields every frame published from now on, each one
// NDJSON line including its newline, and is closed when the subscription ends. cancel ends it and
// may be called more than once. If the subscriber falls SubscriberQueue frames behind, the broker
// drops it: frames is closed and onDrop, when not nil, runs (outside the broker's lock).
// Subscribing to a nil broker yields a closed channel.
func (b *Broker) Subscribe(onDrop func()) (frames <-chan []byte, cancel func()) {
	if b == nil {
		none := make(chan []byte)
		close(none)
		return none, func() {}
	}
	s := &subscription{frames: make(chan []byte, SubscriberQueue), onDrop: onDrop}
	b.mu.Lock()
	b.subs[s] = struct{}{}
	b.mu.Unlock()
	return s.frames, func() {
		b.mu.Lock()
		defer b.mu.Unlock()
		b.remove(s)
	}
}

// remove ends s if it is still subscribed. The caller holds b.mu.
func (b *Broker) remove(s *subscription) bool {
	if _, ok := b.subs[s]; !ok {
		return false
	}
	delete(b.subs, s)
	close(s.frames)
	return true
}

// Publish queues frame for every subscriber and returns how many it was queued for. It never
// blocks: a subscriber whose queue is full is dropped instead.
func (b *Broker) Publish(frame any) int {
	if b == nil {
		return 0
	}
	data, err := json.Marshal(frame)
	if err != nil {
		log.Printf("ipc: cannot encode a %T event: %v", frame, err)
		return 0
	}
	data = append(data, '\n')

	var dropped []*subscription
	b.mu.Lock()
	queued := 0
	for s := range b.subs {
		select {
		case s.frames <- data:
			queued++
		default:
			b.remove(s)
			dropped = append(dropped, s)
		}
	}
	b.mu.Unlock()

	for _, s := range dropped {
		if s.onDrop != nil {
			s.onDrop()
		}
	}
	return queued
}
