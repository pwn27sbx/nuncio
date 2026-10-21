//! Hyprland's window rounding, so the app's corners match the compositor's.
//!
//! Asked once at start, and only under Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` set):
//! `hyprctl -j getoption decoration:rounding` answers `{"option": "decoration:rounding", "int": N,
//! "set": true}`. Anything else (no Hyprland, no `hyprctl`, a slow or failing one, output that is
//! not that JSON) leaves libadwaita's own corners.

use std::ffi::OsStr;
use std::time::Duration;

use nuncio_client::font::{Run, RunError};

/// The program and arguments that ask Hyprland its rounding.
pub const HYPRCTL: (&str, [&str; 3]) = ("hyprctl", ["-j", "getoption", "decoration:rounding"]);

/// How long startup may wait for `hyprctl`.
pub const TIMEOUT: Duration = Duration::from_millis(200);

/// The largest radius taken from Hyprland, in pixels; a larger value is capped.
pub const MAX_RADIUS: u32 = 64;

/// Why Hyprland's rounding is not used.
#[derive(Debug, thiserror::Error)]
pub enum RoundingError {
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("hyprctl answered something other than a rounding: {0:?}")]
    Unexpected(String),
}

/// The window corner radius in pixels: `Ok(None)` when not on Hyprland (nothing is run),
/// `Ok(Some(px))` when Hyprland answered, an error when it was asked and gave no usable answer.
pub fn rounding(
    hyprland_signature: Option<&OsStr>,
    runner: &dyn Run,
) -> Result<Option<u32>, RoundingError> {
    if hyprland_signature.is_none_or(OsStr::is_empty) {
        return Ok(None);
    }
    let (program, args) = HYPRCTL;
    let out = runner.run(program, &args, TIMEOUT)?;
    parse_rounding(&out).map(Some)
}

/// Parse `hyprctl -j getoption decoration:rounding`: its `int` (or, defensively, a `float`
/// rounded), never negative, capped at [`MAX_RADIUS`].
pub fn parse_rounding(json: &[u8]) -> Result<u32, RoundingError> {
    let unexpected = || RoundingError::Unexpected(snippet(json));
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|_| unexpected())?;
    let radius = if let Some(int) = value.get("int").and_then(serde_json::Value::as_i64) {
        int
    } else if let Some(float) = value.get("float").and_then(serde_json::Value::as_f64) {
        if !float.is_finite() {
            return Err(unexpected());
        }
        float.round() as i64
    } else {
        return Err(unexpected());
    };
    if radius < 0 {
        return Err(unexpected());
    }
    Ok(u32::try_from(radius).unwrap_or(u32::MAX).min(MAX_RADIUS))
}

/// A short, single-line excerpt of unexpected output for the one log line.
fn snippet(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::{HYPRCTL, MAX_RADIUS, RoundingError, TIMEOUT, parse_rounding, rounding};
    use nuncio_client::font::{Run, RunError};
    use std::ffi::OsStr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// What Hyprland prints for `hyprctl -j getoption decoration:rounding` (Omarchy sets 0).
    const REAL: &str = r#"{"option": "decoration:rounding", "int": 10, "set": true}"#;

    struct Fake {
        answer: fn() -> Result<Vec<u8>, RunError>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(answer: fn() -> Result<Vec<u8>, RunError>) -> Self {
            Self {
                answer,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Run for Fake {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            timeout: Duration,
        ) -> Result<Vec<u8>, RunError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!((program, args), (HYPRCTL.0, &HYPRCTL.1[..]));
            assert!(timeout <= Duration::from_millis(200), "{timeout:?}");
            (self.answer)()
        }
    }

    fn signature() -> Option<&'static OsStr> {
        Some(OsStr::new("abc_123_456"))
    }

    #[test]
    fn the_rounding_is_hyprlands_int() {
        assert_eq!(parse_rounding(REAL.as_bytes()).expect("parses"), 10);
        assert_eq!(
            parse_rounding(br#"{"option":"decoration:rounding","int":0,"set":false}"#)
                .expect("parses"),
            0
        );
    }

    #[test]
    fn a_float_is_rounded_and_out_of_range_values_are_bounded() {
        assert_eq!(parse_rounding(br#"{"float": 7.6}"#).expect("parses"), 8);
        assert_eq!(
            parse_rounding(br#"{"int": 5000}"#).expect("parses"),
            MAX_RADIUS
        );
        assert!(parse_rounding(br#"{"int": -4}"#).is_err());
    }

    #[test]
    fn a_missing_key_an_error_or_non_json_is_unexpected() {
        for out in [
            &br#"{"option": "decoration:rounding", "set": true}"#[..],
            br#"{"error": "no such option"}"#,
            br#"{"int": "10"}"#,
            b"no such option\n",
            b"",
        ] {
            assert!(
                matches!(parse_rounding(out), Err(RoundingError::Unexpected(_))),
                "{:?}",
                String::from_utf8_lossy(out)
            );
        }
    }

    #[test]
    fn hyprland_is_asked_once_with_a_short_timeout() {
        let fake = Fake::new(|| Ok(REAL.as_bytes().to_vec()));
        assert_eq!(rounding(signature(), &fake).expect("answered"), Some(10));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        assert!(TIMEOUT <= Duration::from_millis(200));
    }

    #[test]
    fn not_on_hyprland_nothing_is_run() {
        let fake = Fake::new(|| panic!("hyprctl must not run outside Hyprland"));
        assert_eq!(rounding(None, &fake).expect("no call"), None);
        assert_eq!(
            rounding(Some(OsStr::new("")), &fake).expect("no call"),
            None
        );
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_slow_missing_or_failing_hyprctl_is_an_error() {
        let slow = Fake::new(|| {
            Err(RunError::TimedOut {
                program: "hyprctl".to_owned(),
                timeout: Duration::from_millis(200),
            })
        });
        assert!(matches!(
            rounding(signature(), &slow),
            Err(RoundingError::Run(RunError::TimedOut { .. }))
        ));
        let missing = Fake::new(|| Err(RunError::NotFound("hyprctl".to_owned())));
        assert!(matches!(
            rounding(signature(), &missing),
            Err(RoundingError::Run(RunError::NotFound(_)))
        ));
        let garbage = Fake::new(|| Ok(b"Couldn't connect to the Hyprland socket\n".to_vec()));
        let err = rounding(signature(), &garbage).expect_err("garbage");
        assert!(err.to_string().contains("Couldn't connect"), "{err}");
    }

    #[test]
    fn each_error_message_is_unchanged() {
        let run = |error| RoundingError::from(error).to_string();
        assert_eq!(
            run(RunError::NotFound("hyprctl".to_owned())),
            "hyprctl is not installed"
        );
        assert_eq!(
            run(RunError::TimedOut {
                program: "hyprctl".to_owned(),
                timeout: Duration::from_millis(200),
            }),
            "hyprctl did not answer within 200ms"
        );
        assert_eq!(
            run(RunError::Io {
                program: "hyprctl".to_owned(),
                source: std::io::Error::other("denied"),
            }),
            "cannot run hyprctl: denied"
        );
        assert_eq!(
            RoundingError::Unexpected("nope".to_owned()).to_string(),
            r#"hyprctl answered something other than a rounding: "nope""#
        );
    }
}
