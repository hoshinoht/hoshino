//! Local-time collection with DST-aware day-boundary math.
use crate::model::{Diagnostic, LocalTime};
use jiff::Zoned;

#[derive(Debug)]
pub struct TimeFacts {
    pub time: LocalTime,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
struct TimeInput {
    local: Option<String>,
    date: Option<String>,
    timezone_name: Option<String>,
    timezone_offset_seconds: Option<i32>,
    instant_seconds: Option<i64>,
    day_start_seconds: Option<i64>,
    next_day_start_seconds: Option<i64>,
}

/// Collect local clock facts using the operating system's configured timezone.
pub fn collect() -> TimeFacts {
    let now = Zoned::now();
    let input = TimeInput {
        local: Some(now.strftime("%H:%M:%S").to_string()),
        date: Some(now.date().to_string()),
        timezone_name: now.time_zone().iana_name().map(str::to_owned),
        timezone_offset_seconds: Some(now.offset().seconds()),
        instant_seconds: Some(now.timestamp().as_second()),
        day_start_seconds: now
            .start_of_day()
            .ok()
            .map(|value| value.timestamp().as_second()),
        next_day_start_seconds: now
            .tomorrow()
            .and_then(|value| value.start_of_day())
            .ok()
            .map(|value| value.timestamp().as_second()),
    };
    from_input(input)
}

fn from_input(input: TimeInput) -> TimeFacts {
    let day_progress_percent = match (
        input.instant_seconds,
        input.day_start_seconds,
        input.next_day_start_seconds,
    ) {
        (Some(now), Some(start), Some(end)) => day_progress_percent(now, start, end),
        _ => None,
    };
    TimeFacts {
        time: LocalTime {
            local: input.local,
            date: input.date,
            timezone_name: input.timezone_name,
            timezone_offset_seconds: input.timezone_offset_seconds,
            day_progress_percent,
        },
        diagnostics: Vec::new(),
    }
}

fn day_progress_percent(now: i64, day_start: i64, next_day_start: i64) -> Option<f64> {
    let day_seconds = next_day_start.checked_sub(day_start)?;
    let elapsed = now.checked_sub(day_start)?;
    (day_seconds > 0 && (0..=day_seconds).contains(&elapsed))
        .then_some((elapsed as f64 / day_seconds as f64) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(now: i64, start: i64, end: i64) -> TimeInput {
        TimeInput {
            local: Some("12:00:00".into()),
            date: Some("2024-02-29".into()),
            timezone_name: Some("Test/Zone".into()),
            timezone_offset_seconds: Some(0),
            instant_seconds: Some(now),
            day_start_seconds: Some(start),
            next_day_start_seconds: Some(end),
        }
    }

    #[test]
    fn day_progress_uses_actual_dst_day_lengths_and_midnight() {
        assert_eq!(day_progress_percent(0, 0, 82_800), Some(0.0));
        assert_eq!(day_progress_percent(41_400, 0, 82_800), Some(50.0));
        assert_eq!(day_progress_percent(45_000, 0, 90_000), Some(50.0));
        assert_eq!(day_progress_percent(86_400, 0, 86_400), Some(100.0));
    }

    #[test]
    fn leap_day_and_timezone_unavailability_are_preserved() {
        let mut value = input(43_200, 0, 86_400);
        value.timezone_name = None;
        value.timezone_offset_seconds = None;
        let facts = from_input(value);
        assert_eq!(facts.time.date.as_deref(), Some("2024-02-29"));
        assert_eq!(facts.time.timezone_name, None);
        assert_eq!(facts.time.timezone_offset_seconds, None);
        assert_eq!(facts.time.day_progress_percent, Some(50.0));
    }

    #[test]
    fn invalid_boundaries_are_unavailable() {
        assert_eq!(day_progress_percent(0, 1, 1), None);
        assert_eq!(day_progress_percent(-1, 0, 86_400), None);
        assert_eq!(day_progress_percent(86_401, 0, 86_400), None);
    }

    #[test]
    fn zoned_boundaries_cover_real_dst_transition() {
        let spring: Zoned = "2024-03-10 12:00[America/New_York]".parse().unwrap();
        let spring_start = spring.start_of_day().unwrap().timestamp().as_second();
        let spring_end = spring
            .tomorrow()
            .unwrap()
            .start_of_day()
            .unwrap()
            .timestamp()
            .as_second();
        assert_eq!(spring_end - spring_start, 82_800);
        let fall: Zoned = "2024-11-03 12:00[America/New_York]".parse().unwrap();
        let fall_start = fall.start_of_day().unwrap().timestamp().as_second();
        let fall_end = fall
            .tomorrow()
            .unwrap()
            .start_of_day()
            .unwrap()
            .timestamp()
            .as_second();
        assert_eq!(fall_end - fall_start, 90_000);
    }
}
