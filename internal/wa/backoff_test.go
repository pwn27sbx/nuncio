package wa

import (
	"math/rand/v2"
	"testing"
	"time"
)

// backoffClock is a manual clock: time only moves when the test advances it.
type backoffClock struct{ t time.Time }

func (c *backoffClock) Now() time.Time          { return c.t }
func (c *backoffClock) Advance(d time.Duration) { c.t = c.t.Add(d) }

func newTestBackoff(r func() float64) (*Backoff, *backoffClock) {
	clock := &backoffClock{t: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)}
	b := NewBackoff()
	b.Rand = r
	b.Now = clock.Now
	return b, clock
}

func halfJitter() float64 { return 0.5 }

func mustNextDelay(t *testing.T, b *Backoff) time.Duration {
	t.Helper()
	d, ok := b.Next()
	if !ok {
		t.Fatalf("Next() refused an attempt; stopped reason %q", b.Stopped())
	}
	return d
}

func TestBackoffDelaysGrow(t *testing.T) {
	b, _ := newTestBackoff(halfJitter)
	// Equal jitter with r = 0.5: delay = 3/4 of the exponential step.
	want := []time.Duration{
		1500 * time.Millisecond,
		3 * time.Second,
		6 * time.Second,
		12 * time.Second,
		24 * time.Second,
	}
	for i, w := range want {
		if got := mustNextDelay(t, b); got != w {
			t.Fatalf("attempt %d: delay %v, want %v", i+1, got, w)
		}
	}
}

func TestBackoffJitterStaysWithinBounds(t *testing.T) {
	r := rand.New(rand.NewPCG(1, 2))
	distinct := map[time.Duration]bool{}
	for range 200 {
		b, _ := newTestBackoff(r.Float64)
		step := b.Base
		for attempt := 1; attempt <= 6; attempt++ {
			d := mustNextDelay(t, b)
			if d < step/2 || d >= step {
				t.Fatalf("attempt %d: delay %v outside [%v, %v)", attempt, d, step/2, step)
			}
			if attempt == 3 {
				distinct[d] = true
			}
			step *= 2
		}
	}
	if len(distinct) < 50 {
		t.Fatalf("only %d distinct delays over 200 draws: no jitter", len(distinct))
	}
}

func TestBackoffCapsAtTheCeiling(t *testing.T) {
	r := rand.New(rand.NewPCG(3, 4))
	b, _ := newTestBackoff(r.Float64)
	for attempt := 1; attempt <= 200; attempt++ {
		d := mustNextDelay(t, b)
		if d > b.Ceiling {
			t.Fatalf("attempt %d: delay %v above the ceiling %v", attempt, d, b.Ceiling)
		}
		if attempt > 20 && d < b.Ceiling/2 {
			t.Fatalf("attempt %d: delay %v fell below halfJitter the ceiling", attempt, d)
		}
	}
}

func TestBackoffResetsOnlyAfterAStableConnection(t *testing.T) {
	b, clock := newTestBackoff(halfJitter)
	for range 4 {
		mustNextDelay(t, b)
	}

	// A short connection must not reset: the next delay keeps growing.
	b.Connected()
	clock.Advance(b.Stable - time.Second)
	if got, want := mustNextDelay(t, b), 24*time.Second; got != want {
		t.Fatalf("after a short connection: delay %v, want %v (no reset)", got, want)
	}

	// A connection that lasts the stable threshold resets to the base step.
	b.Connected()
	clock.Advance(b.Stable)
	if got, want := mustNextDelay(t, b), 1500*time.Millisecond; got != want {
		t.Fatalf("after a stable connection: delay %v, want %v (reset)", got, want)
	}
}

func TestBackoffStopIsTerminal(t *testing.T) {
	b, clock := newTestBackoff(halfJitter)
	mustNextDelay(t, b)
	if b.Stopped() != "" {
		t.Fatalf("Stopped() = %q before Stop", b.Stopped())
	}

	b.Stop("logged out")
	if got := b.Stopped(); got != "logged out" {
		t.Fatalf("Stopped() = %q, want %q", got, "logged out")
	}
	for i := range 3 {
		if d, ok := b.Next(); ok {
			t.Fatalf("Next() #%d after Stop yielded an attempt in %v", i+1, d)
		}
	}
	// Neither a connection nor a stable stretch revives a stopped policy.
	b.Connected()
	clock.Advance(b.Stable * 2)
	if d, ok := b.Next(); ok {
		t.Fatalf("Next() after Stop and a stable connection yielded an attempt in %v", d)
	}

	// Only an explicit Reset (e.g. after a fresh link) allows attempts again.
	b.Reset()
	if b.Stopped() != "" {
		t.Fatalf("Stopped() = %q after Reset", b.Stopped())
	}
	if got, want := mustNextDelay(t, b), 1500*time.Millisecond; got != want {
		t.Fatalf("after Reset: delay %v, want %v", got, want)
	}
}
