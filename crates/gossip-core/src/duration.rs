//! Durations as the command line and the config file write them: `30s`,
//! `5m`, `1h`, `2d`, or several in a row such as `1h30m`.

use std::time::Duration;

use thiserror::Error;

/// A text that is not a duration.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "`{0}` is not a duration; write a number and a unit (s, m, h or d), such as `30s`, `5m` or `1h`"
)]
pub struct DurationError(pub String);

/// Reads a duration such as `30s`, `5m`, `1h` or `1h30m`.
pub fn parse_duration(input: &str) -> Result<Duration, DurationError> {
    let error = || DurationError(input.to_string());
    let text = input.trim();
    if text.is_empty() {
        return Err(error());
    }

    let mut total: u64 = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(error());
        }
        let number: u64 = rest[..digits].parse().map_err(|_| error())?;
        rest = &rest[digits..];
        let unit = rest.chars().next().ok_or_else(error)?;
        let seconds_per_unit: u64 = match unit {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            'd' => 24 * 60 * 60,
            _ => return Err(error()),
        };
        rest = &rest[unit.len_utf8()..];
        total = number
            .checked_mul(seconds_per_unit)
            .and_then(|seconds| total.checked_add(seconds))
            .ok_or_else(error)?;
    }
    Ok(Duration::from_secs(total))
}

/// Writes a duration the way [`parse_duration`] reads it, in whole seconds.
pub fn format_duration(duration: Duration) -> String {
    let mut seconds = duration.as_secs();
    if seconds == 0 {
        return "0s".to_string();
    }
    let mut text = String::new();
    for (unit, size) in [('d', 24 * 60 * 60), ('h', 60 * 60), ('m', 60), ('s', 1)] {
        let count = seconds / size;
        seconds %= size;
        if count > 0 {
            text.push_str(&count.to_string());
            text.push(unit);
        }
    }
    text
}

/// An optional duration as an optional string in serde.
pub(crate) mod optional {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    use super::{format_duration, parse_duration};

    pub fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(duration) => serializer.serialize_some(&format_duration(*duration)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        let text = Option::<String>::deserialize(deserializer)?;
        text.map(|text| parse_duration(&text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_units() {
        assert_eq!(parse_duration("30s"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Ok(Duration::from_secs(300)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
        assert_eq!(parse_duration("2d"), Ok(Duration::from_secs(172_800)));
        assert_eq!(parse_duration("0s"), Ok(Duration::ZERO));
    }

    #[test]
    fn several_parts_add_up() {
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_duration(" 1m30s "), Ok(Duration::from_secs(90)));
    }

    #[test]
    fn what_is_not_a_duration() {
        for text in ["", " ", "5", "m", "5x", "5 m", "-5m", "1.5h", "5m3", "five"] {
            assert!(parse_duration(text).is_err(), "`{text}` should not parse");
        }
        assert!(parse_duration("99999999999999999999s").is_err());
        assert!(parse_duration("999999999999999999d").is_err());
    }

    #[test]
    fn formatting_parses_back() {
        for (seconds, text) in [
            (0, "0s"),
            (30, "30s"),
            (300, "5m"),
            (5400, "1h30m"),
            (86_400, "1d"),
            (90_061, "1d1h1m1s"),
        ] {
            let duration = Duration::from_secs(seconds);
            assert_eq!(format_duration(duration), text);
            assert_eq!(parse_duration(text), Ok(duration));
        }
    }
}
