//! How dates and times are written (Options → Regional): as the system's region writes them
//! (Windows' region settings, elsewhere the language's convention), or the user's own choice of
//! date order, 12- or 24-hour clock and first day of the week. The download list, the
//! Scheduler's date and time boxes and its calendar read it here.

use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// The order of a date's day, month and year.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateOrder {
    Dmy,
    #[default]
    Mdy,
    Ymd,
}

/// What Options → Regional holds (saved in the settings by these names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Regional {
    /// Write dates and times the user's way rather than the system's.
    pub custom: bool,
    pub order: DateOrder,
    pub clock24: bool,
    /// The first day of the week, 0 = Sunday … 6 = Saturday.
    pub week_start: u8,
}

impl Default for Regional {
    fn default() -> Self {
        let system = system();
        Regional {
            custom: false,
            ..system
        }
    }
}

/// The formats in use: the custom ones, or the system's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formats {
    /// The user's own choice (else the system's).
    pub custom: bool,
    pub order: DateOrder,
    pub clock24: bool,
    pub week_start: u8,
}

static CURRENT: RwLock<Option<Formats>> = RwLock::new(None);

/// Use `r` from now on (the settings loaded, or Options → OK).
pub fn set(r: &Regional) {
    *CURRENT.write().unwrap() = Some(effective(r));
}

/// The formats in use now (the system's until `set`).
pub fn current() -> Formats {
    CURRENT
        .read()
        .unwrap()
        .unwrap_or_else(|| effective(&Regional::default()))
}

pub fn effective(r: &Regional) -> Formats {
    let custom = r.custom;
    let r = if r.custom { *r } else { system() };
    Formats {
        custom,
        order: r.order,
        clock24: r.clock24,
        week_start: r.week_start.min(6),
    }
}

/// The system's region formats: Windows' settings; elsewhere the language's convention
/// (`LC_ALL`, `LC_TIME`, `LANG`): the United States and a few others month first and 12-hour,
/// East Asia and Hungary year first, the rest day first and 24-hour.
pub fn system() -> Regional {
    #[cfg(windows)]
    if let Some((order, clock24, week_start)) = crate::win32::region_formats() {
        return Regional {
            custom: false,
            order,
            clock24,
            week_start,
        };
    }
    let lang = ["LC_ALL", "LC_TIME", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let lang = lang.split('.').next().unwrap_or("").replace('-', "_");
    let us_like = ["en_us", "en_ph", "en_ca", "es_us"].contains(&lang.as_str());
    let year_first = ["ja", "zh", "ko", "hu", "lt", "mn"]
        .iter()
        .any(|p| lang.starts_with(p));
    Regional {
        custom: false,
        order: if us_like {
            DateOrder::Mdy
        } else if year_first {
            DateOrder::Ymd
        } else {
            DateOrder::Dmy
        },
        clock24: !(us_like || lang.starts_with("en_au") || lang.starts_with("en_in")),
        week_start: if us_like || lang.starts_with("ja") || lang.starts_with("he") {
            0
        } else {
            1
        },
    }
}

/// `t` as a short date and time, in the formats in use: `30/10/2026 21:41:00`,
/// `10/30/2026 9:41:00 PM`, `2026-10-30 21:41:00`.
pub fn short_date_time(t: chrono::NaiveDateTime) -> String {
    format_with(current(), t)
}

/// [`short_date_time`] in the formats `f`.
pub fn format_with(f: Formats, t: chrono::NaiveDateTime) -> String {
    use chrono::Timelike;
    format!(
        "{} {}",
        date_with(f, t.date()),
        time_text(t.hour(), t.minute(), t.second(), f.clock24)
    )
}

/// A date alone, in the current formats: `31/10/2026`.
pub fn short_date(d: chrono::NaiveDate) -> String {
    date_with(current(), d)
}

fn date_with(f: Formats, t: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let (y, m, d) = (t.year(), t.month(), t.day());
    match f.order {
        DateOrder::Dmy => format!("{d:02}/{m:02}/{y}"),
        DateOrder::Mdy => format!("{m:02}/{d:02}/{y}"),
        DateOrder::Ymd => format!("{y}-{m:02}-{d:02}"),
    }
}

/// A time of day: `21:41:00`, or `9:41:00 PM` (AM and PM translated).
pub fn time_text(h: u32, m: u32, s: u32, clock24: bool) -> String {
    if clock24 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        let h12 = if h.is_multiple_of(12) { 12 } else { h % 12 };
        let mark = if h < 12 {
            crate::i18n::t("AM")
        } else {
            crate::i18n::t("PM")
        };
        format!("{h12}:{m:02}:{s:02} {mark}")
    }
}

/// A time of day to the minute, in the current formats: `14:30`, or `2:30 PM` (a snooze's end,
/// #741).
pub fn clock_time(h: u32, m: u32) -> String {
    clock_time_with(current().clock24, h, m)
}

fn clock_time_with(clock24: bool, h: u32, m: u32) -> String {
    if clock24 {
        format!("{h:02}:{m:02}")
    } else {
        let h12 = if h.is_multiple_of(12) { 12 } else { h % 12 };
        let mark = if h < 12 {
            crate::i18n::t("AM")
        } else {
            crate::i18n::t("PM")
        };
        format!("{h12}:{m:02} {mark}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_time_to_the_minute() {
        assert_eq!(clock_time_with(true, 14, 30), "14:30");
        assert_eq!(clock_time_with(true, 7, 5), "07:05");
        assert_eq!(clock_time_with(false, 14, 30), "2:30 PM");
        assert_eq!(clock_time_with(false, 0, 0), "12:00 AM");
    }

    #[test]
    fn custom_formats_win_over_the_system() {
        let r = Regional {
            custom: true,
            order: DateOrder::Ymd,
            clock24: true,
            week_start: 9,
        };
        let f = effective(&r);
        assert_eq!(f.order, DateOrder::Ymd);
        assert!(f.clock24);
        assert_eq!(f.week_start, 6, "clamped to a weekday");
        let system = system();
        let auto = effective(&Regional { custom: false, ..r });
        assert_eq!(auto.order, system.order, "not custom: the system's");
    }

    #[test]
    fn dates_and_times_are_written_in_each_order() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 30)
            .unwrap()
            .and_hms_opt(21, 41, 5)
            .unwrap();
        let cases = [
            (DateOrder::Dmy, true, "30/10/2026 21:41:05"),
            (DateOrder::Ymd, true, "2026-10-30 21:41:05"),
            (DateOrder::Mdy, false, "10/30/2026 9:41:05 PM"),
        ];
        for (order, clock24, want) in cases {
            set(&Regional {
                custom: true,
                order,
                clock24,
                week_start: 1,
            });
            assert_eq!(short_date_time(t), want);
        }
        assert_eq!(time_text(0, 5, 0, false), "12:05:00 AM");
        assert_eq!(time_text(12, 0, 0, false), "12:00:00 PM");
    }
}
