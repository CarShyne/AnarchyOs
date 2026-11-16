//! Wall-clock time for the desktop. The hardware clock is read twice a
//! second into a cache, so drawing the menu bar never touches hardware.

use crate::rtc;
use crate::theme;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

#[derive(Clone, Copy, PartialEq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Days since 1970-01-01 for a calendar date.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y as i32, m, d)
}

impl DateTime {
    /// 0 is Monday.
    pub fn weekday(&self) -> usize {
        (days_from_civil(self.year, self.month, self.day) + 3).rem_euclid(7) as usize
    }

    /// The same instant moved by a whole number of hours.
    pub fn shifted(&self, hours: i32) -> DateTime {
        let total = self.hour as i64 + hours as i64;
        let days = days_from_civil(self.year, self.month, self.day) + total.div_euclid(24);
        let (year, month, day) = civil_from_days(days);
        DateTime { year, month, day, hour: total.rem_euclid(24) as u32, ..*self }
    }

    fn pack(&self) -> u64 {
        (self.year as u64) << 40
            | (self.month as u64) << 32
            | (self.day as u64) << 24
            | (self.hour as u64) << 16
            | (self.minute as u64) << 8
            | self.second as u64
    }

    fn unpack(v: u64) -> DateTime {
        DateTime {
            year: (v >> 40) as i32,
            month: ((v >> 32) & 0xFF) as u32,
            day: ((v >> 24) & 0xFF) as u32,
            hour: ((v >> 16) & 0xFF) as u32,
            minute: ((v >> 8) & 0xFF) as u32,
            second: (v & 0xFF) as u32,
        }
    }
}

static CACHE: AtomicU64 = AtomicU64::new(0);

/// Re-read the hardware clock. Called from the main loop, not from drawing.
pub fn refresh() {
    CACHE.store(rtc::read().pack(), Ordering::Relaxed);
}

/// The current time with the user's offset applied.
pub fn now() -> DateTime {
    let v = CACHE.load(Ordering::Relaxed);
    let raw = if v == 0 {
        DateTime { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }
    } else {
        DateTime::unpack(v)
    };
    raw.shifted(theme::cfg().tz_offset)
}

/// "19:24", "7:24 PM", with seconds when asked for.
pub fn time_string(seconds: bool) -> String {
    let t = now();
    let c = theme::cfg();
    let secs = if seconds { format!(":{:02}", t.second) } else { String::new() };
    if c.clock_24h {
        format!("{:02}:{:02}{}", t.hour, t.minute, secs)
    } else {
        let h = match t.hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{}:{:02}{} {}", h, t.minute, secs, if t.hour < 12 { "AM" } else { "PM" })
    }
}

/// "Mon 16 Nov"
pub fn short_date() -> String {
    let t = now();
    format!("{} {} {}", &WEEKDAYS[t.weekday()][..3], t.day, &MONTHS[(t.month - 1) as usize][..3])
}

/// "Monday, 16 November 2026"
pub fn long_date() -> String {
    let t = now();
    format!("{}, {} {} {}", WEEKDAYS[t.weekday()], t.day, MONTHS[(t.month - 1) as usize], t.year)
}

/// Changes whenever the text shown in the menu bar would change.
pub fn stamp() -> u64 {
    let t = now();
    let c = theme::cfg();
    let base = (t.day as u64) << 16 | (t.hour as u64) << 8 | t.minute as u64;
    if c.show_seconds {
        base << 8 | t.second as u64
    } else {
        base
    }
}
