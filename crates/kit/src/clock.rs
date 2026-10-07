//! The process clock, and the HTTP layer's date arithmetic and formats.
//!
//! Set `MATCHBOX_FROZEN_TIME` (an RFC 3339 timestamp such as `2024-06-01T12:00:00Z`) to pin
//! `now()` for the whole process; [`from_env`] reads it at boot.

use std::sync::Arc;

use jiff::Timestamp;

pub use rails_compat::clock::{Clock, SharedClock, SystemClock, TestClock};

pub const FROZEN_TIME_ENV: &str = "MATCHBOX_FROZEN_TIME";

/// The process clock: frozen at `MATCHBOX_FROZEN_TIME` when set, the system clock otherwise.
pub fn from_env() -> anyhow::Result<SharedClock> {
    match rails_compat::env::var(FROZEN_TIME_ENV) {
        Ok(value) if !value.trim().is_empty() => {
            let now: Timestamp =
                value.trim().parse().map_err(|e| anyhow::anyhow!("{FROZEN_TIME_ENV}={value:?} is not an RFC 3339 timestamp: {e}"))?;
            Ok(Arc::new(TestClock::frozen_at(now)))
        }
        _ => Ok(Arc::new(SystemClock)),
    }
}

/// `n.years.from_now` as ActiveSupport computes it: calendar years in UTC.
pub fn years_from(now: Timestamp, years: i64) -> Timestamp {
    now.to_zoned(jiff::tz::TimeZone::UTC).checked_add(jiff::Span::new().years(years)).map(|z| z.timestamp()).unwrap_or(now)
}

/// An HTTP date (`Time#httpdate`): `Thu, 01 Jan 1970 00:00:00 GMT`.
pub fn httpdate(at: Timestamp) -> String {
    jiff::fmt::rfc2822::DateTimePrinter::new().timestamp_to_rfc9110_string(&at).expect("valid http date")
}

/// Parse an HTTP date (`Time.httpdate` / `Time.rfc2822`), `None` when malformed.
pub fn parse_httpdate(value: &str) -> Option<Timestamp> {
    jiff::fmt::rfc2822::DateTimeParser::new().parse_timestamp(value.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_years_is_calendar_years() {
        let t: Timestamp = "2024-02-29T00:00:00Z".parse().unwrap();
        assert_eq!(years_from(t, 20).to_string(), "2044-02-29T00:00:00Z");
    }

    #[test]
    fn http_dates() {
        assert_eq!(httpdate(Timestamp::UNIX_EPOCH), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(parse_httpdate("Thu, 01 Jan 1970 00:00:00 GMT"), Some(Timestamp::UNIX_EPOCH));
        assert_eq!(parse_httpdate("garbage"), None);
    }
}
