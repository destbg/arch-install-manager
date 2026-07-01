pub fn format_build_date(timestamp: i64) -> String {
    use chrono::{Local, TimeZone, Utc};

    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;

    let absolute_date = || {
        Local
            .timestamp_opt(timestamp, 0)
            .single()
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    };

    let diff = Utc::now().timestamp() - timestamp;
    if diff < 0 || diff >= WEEK {
        return absolute_date();
    }

    if diff < HOUR {
        let minutes = diff / 60;
        if minutes < 1 {
            return "just now".to_string();
        }
        return format!("{} minute{} ago", minutes, plural(minutes));
    }

    if diff < DAY {
        let hours = diff / HOUR;
        return format!("{} hour{} ago", hours, plural(hours));
    }

    let days = diff / DAY;
    return format!("{} day{} ago", days, plural(days));
}

pub fn plural(n: i64) -> &'static str {
    return if n == 1 { "" } else { "s" };
}
