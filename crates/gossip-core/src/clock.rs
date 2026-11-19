//! The time of day, as everything that stamps a record of its own or
//! measures an age reads it.
//!
//! It is the system's clock. The one exception is for the generator of the
//! README (`cargo xtask readme`), whose output has to be the same on every
//! run: when the environment variable `GOSSIP_CLOCK` names a file, the time
//! is the RFC 3339 time that file holds, read anew every time it is asked
//! for, so that whoever runs the program can move the clock as it goes. A
//! file that cannot be read, or holds no time, leaves the system's clock in
//! charge. Nothing else in the program knows about it.

use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};

/// The environment variable that names the file the time is read from.
pub const CLOCK_FILE: &str = "GOSSIP_CLOCK";

/// The time now.
pub fn now() -> DateTime<Utc> {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    let file = FILE.get_or_init(|| {
        std::env::var_os(CLOCK_FILE)
            .filter(|file| !file.is_empty())
            .map(PathBuf::from)
    });
    file.as_deref()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| parse(&text))
        .unwrap_or_else(Utc::now)
}

/// The time a clock file holds.
fn parse(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_clock_file_holds_one_time() {
        let time = Utc.with_ymd_and_hms(2026, 11, 18, 14, 2, 11).unwrap();
        assert_eq!(parse("2026-11-18T14:02:11Z\n"), Some(time));
        assert_eq!(parse("  2026-11-18T15:02:11+01:00"), Some(time));
        assert_eq!(parse(""), None);
        assert_eq!(parse("soon"), None);
    }

    #[test]
    fn without_a_clock_file_the_time_is_the_systems() {
        // The tests run without the variable.
        assert!(std::env::var_os(CLOCK_FILE).is_none());
        let before = Utc::now();
        let now = now();
        assert!(now >= before && now <= Utc::now());
    }
}
