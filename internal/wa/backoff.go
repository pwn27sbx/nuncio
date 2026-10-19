package wa

import (
	"math/rand/v2"
	"time"
)

// Backoff is the reconnect policy: capped exponential backoff with equal jitter, reset only after
// a stable connection, and a terminal stop after a logout or ban. It is pure (no goroutines, no
// timers): the caller asks Next for the delay before each reconnect attempt and waits itself.
//
// Equal jitter: the n-th attempt (n from 0) has the step s = min(Base*2^n, Ceiling) and waits
// s/2 + Rand()*s/2, so every delay is in [s/2, s). Unlike full jitter (Rand()*s), a delay never
// collapses towards zero, which keeps a fleet of failed connections from hammering the server.
type Backoff struct {
	Base    time.Duration    // first step (default 2 s)
	Ceiling time.Duration    // largest step; no delay reaches it (default 5 min)
	Stable  time.Duration    // a connection lasting this long resets the steps (default 1 min)
	Rand    func() float64   // jitter source in [0, 1); injected by tests
	Now     func() time.Time // clock for the stable threshold; injected by tests

	attempt   int
	up        bool
	upSince   time.Time
	stoppedBy string
}

// NewBackoff returns the policy with its defaults, the process RNG and the wall clock.
func NewBackoff() *Backoff {
	return &Backoff{
		Base:    2 * time.Second,
		Ceiling: 5 * time.Minute,
		Stable:  time.Minute,
		Rand:    rand.Float64,
		Now:     time.Now,
	}
}

// Next reports how long to wait before the next reconnect attempt, after a failed attempt or a
// dropped connection. ok is false once Stop was called: the caller must not reconnect.
func (b *Backoff) Next() (delay time.Duration, ok bool) {
	if b.stoppedBy != "" {
		return 0, false
	}
	if b.up {
		if b.Now().Sub(b.upSince) >= b.Stable {
			b.attempt = 0
		}
		b.up = false
	}
	step := b.Base
	for i := 0; i < b.attempt && step < b.Ceiling; i++ {
		step *= 2
	}
	step = min(step, b.Ceiling)
	b.attempt++
	half := step / 2
	return half + time.Duration(b.Rand()*float64(step-half)), true
}

// Connected records that a connection came up; it counts as stable once it outlives Stable.
func (b *Backoff) Connected() {
	b.up = true
	b.upSince = b.Now()
}

// Stop ends the policy for good (logout, ban): Next refuses until Reset. reason must be non-empty.
func (b *Backoff) Stop(reason string) {
	if reason == "" {
		reason = "stopped"
	}
	b.stoppedBy = reason
}

// Stopped returns the reason given to Stop, or "" while reconnecting is allowed.
func (b *Backoff) Stopped() string { return b.stoppedBy }

// Reset clears the steps and any stop, e.g. after the user links the device again.
func (b *Backoff) Reset() {
	b.attempt = 0
	b.up = false
	b.stoppedBy = ""
}
