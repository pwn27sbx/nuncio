//! When to try `nunciod` again after the connection was lost, for every client (the terminal
//! client's tests pin the schedule). Pure: the caller passes the time, so the policy is tested
//! without sleeping.

use std::time::{Duration, Instant};

/// The first wait after a loss.
pub const FIRST_DELAY: Duration = Duration::from_millis(500);
/// The longest wait between two attempts.
pub const MAX_DELAY: Duration = Duration::from_secs(5);

/// A doubling delay with a cap: 0.5 s, 1 s, 2 s, 4 s, 5 s, 5 s, ...
#[derive(Debug, Clone)]
pub struct Backoff {
    first: Duration,
    cap: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(first: Duration, cap: Duration) -> Self {
        Self {
            first,
            cap,
            next: first,
        }
    }

    /// The wait before the next attempt; each call doubles the one after, up to the cap.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.cap);
        delay
    }

    /// Start again from the first delay (after a successful connect).
    pub fn reset(&mut self) {
        self.next = self.first;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(FIRST_DELAY, MAX_DELAY)
    }
}

/// The reconnect schedule: connected, or waiting for the next attempt.
#[derive(Debug, Clone)]
pub struct Reconnect {
    backoff: Backoff,
    at: Option<Instant>,
}

impl Reconnect {
    pub fn new(backoff: Backoff) -> Self {
        Self { backoff, at: None }
    }

    /// Whether the connection is lost and an attempt is scheduled.
    pub fn is_reconnecting(&self) -> bool {
        self.at.is_some()
    }

    /// The connection was lost at `now`: schedule the first attempt. A loss noticed while
    /// already reconnecting keeps the attempt already scheduled.
    pub fn lost(&mut self, now: Instant) {
        if self.at.is_none() {
            self.at = Some(now + self.backoff.next_delay());
        }
    }

    /// Whether an attempt should be made at `now`.
    pub fn due(&self, now: Instant) -> bool {
        self.at.is_some_and(|at| now >= at)
    }

    /// How long from `now` until the scheduled attempt (zero once it is due); `None` while
    /// connected. For a client that sleeps until then instead of polling [`Reconnect::due`].
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.at.map(|at| at.saturating_duration_since(now))
    }

    /// The attempt made at `now` failed: wait longer before the next one.
    pub fn failed(&mut self, now: Instant) {
        self.at = Some(now + self.backoff.next_delay());
    }

    /// Connected again: nothing is scheduled and the next loss starts from the first delay.
    pub fn succeeded(&mut self) {
        self.at = None;
        self.backoff.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_runs_down_to_the_scheduled_attempt_and_is_none_while_connected() {
        let t0 = Instant::now();
        let mut reconnect = Reconnect::new(Backoff::default());
        assert_eq!(reconnect.wait(t0), None);
        reconnect.lost(t0);
        assert_eq!(reconnect.wait(t0), Some(FIRST_DELAY));
        assert_eq!(
            reconnect.wait(t0 + Duration::from_millis(200)),
            Some(Duration::from_millis(300))
        );
        assert_eq!(reconnect.wait(t0 + MAX_DELAY), Some(Duration::ZERO), "due");
        reconnect.succeeded();
        assert_eq!(reconnect.wait(t0), None);
    }
}
