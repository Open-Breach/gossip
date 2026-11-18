//! Ages as `kubectl` writes them: `45s`, `5m30s`, `3h12m`, `5d`.

use chrono::{DateTime, Utc};

/// The age of something created at `created`, as `kubectl` would show it.
pub fn age_since(created: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    match created {
        Some(created) => human_duration(now.signed_duration_since(created).num_seconds()),
        None => "<unknown>".to_string(),
    }
}

/// A number of seconds in the short form of `kubectl`'s age column: two
/// units while that is still exact enough to matter, one unit after.
pub fn human_duration(seconds: i64) -> String {
    // A clock that is a little off can put the creation in the future.
    if seconds < -1 {
        return "<invalid>".to_string();
    }
    if seconds < 0 {
        return "0s".to_string();
    }
    if seconds < 60 * 2 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 10 {
        let rest = seconds % 60;
        return if rest == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m{rest}s")
        };
    }
    if minutes < 60 * 3 {
        return format!("{minutes}m");
    }
    let hours = seconds / 3600;
    if hours < 8 {
        let rest = minutes % 60;
        return if rest == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{rest}m")
        };
    }
    if hours < 48 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if hours < 24 * 8 {
        let rest = hours % 24;
        return if rest == 0 {
            format!("{days}d")
        } else {
            format!("{days}d{rest}h")
        };
    }
    if hours < 24 * 365 * 2 {
        return format!("{days}d");
    }
    let years = days / 365;
    if hours < 24 * 365 * 8 {
        let rest = days % 365;
        return if rest == 0 {
            format!("{years}y")
        } else {
            format!("{years}y{rest}d")
        };
    }
    format!("{years}y")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn ages_as_kubectl_writes_them() {
        const MINUTE: i64 = 60;
        const HOUR: i64 = 60 * MINUTE;
        const DAY: i64 = 24 * HOUR;
        for (seconds, text) in [
            (-5, "<invalid>"),
            (-1, "0s"),
            (0, "0s"),
            (45, "45s"),
            (119, "119s"),
            (2 * MINUTE, "2m"),
            (5 * MINUTE + 30, "5m30s"),
            (10 * MINUTE + 30, "10m"),
            (179 * MINUTE, "179m"),
            (3 * HOUR, "3h"),
            (3 * HOUR + 12 * MINUTE + 5, "3h12m"),
            (8 * HOUR + 30 * MINUTE, "8h"),
            (47 * HOUR, "47h"),
            (2 * DAY, "2d"),
            (5 * DAY + 7 * HOUR, "5d7h"),
            (8 * DAY + 7 * HOUR, "8d"),
            (400 * DAY, "400d"),
            (2 * 365 * DAY, "2y"),
            (2 * 365 * DAY + 40 * DAY, "2y40d"),
            (9 * 365 * DAY + 40 * DAY, "9y"),
        ] {
            assert_eq!(human_duration(seconds), text, "{seconds} seconds");
        }
    }

    #[test]
    fn the_age_of_something() {
        let now = Utc.with_ymd_and_hms(2026, 11, 18, 14, 0, 0).unwrap();
        let created = Utc.with_ymd_and_hms(2026, 11, 13, 7, 0, 0).unwrap();
        assert_eq!(age_since(Some(created), now), "5d7h");
        assert_eq!(age_since(None, now), "<unknown>");
    }
}
