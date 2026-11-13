//! Time formatting helpers. Timestamps from the server are Unix seconds and
//! are shown in the user's local time zone.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, NaiveDate, Utc};

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn local(ts: u64) -> DateTime<Local> {
    DateTime::<Utc>::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is valid"))
        .with_timezone(&Local)
}

pub fn day_key(ts: u64) -> NaiveDate {
    local(ts).date_naive()
}

/// Short time for hover tooltips and message gutters, e.g. "14:05".
pub fn time_label(ts: u64) -> String {
    local(ts).format("%H:%M").to_string()
}

/// Full stamp shown next to a message author, e.g. "Today at 14:05".
pub fn stamp_label(ts: u64) -> String {
    let day = day_key(ts);
    let today = Local::now().date_naive();
    let time = time_label(ts);
    if day == today {
        format!("Today at {time}")
    } else if Some(day) == today.pred_opt() {
        format!("Yesterday at {time}")
    } else {
        format!("{} {time}", local(ts).format("%m/%d/%Y"))
    }
}

/// Date divider label, e.g. "Today", "Yesterday", or "Thursday, November 12, 2026".
pub fn day_label(ts: u64) -> String {
    let day = day_key(ts);
    let today = Local::now().date_naive();
    if day == today {
        "Today".into()
    } else if Some(day) == today.pred_opt() {
        "Yesterday".into()
    } else {
        local(ts).format("%A, %B %-d, %Y").to_string()
    }
}
