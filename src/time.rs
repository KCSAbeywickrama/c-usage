//! Timestamps. Adapted from claude-find's time.rs.
//!
//! Transcript timestamps are UTC ISO-8601; everything displayed is local time;
//! and /usage prints its reset time as a wall-clock time in a named zone. libc's
//! `localtime_r`/`mktime` give zone-and-DST-aware conversion for all three
//! without pulling in a date crate.

// The libc crate does not expose tzset() on every target, and glibc's
// localtime_r - unlike the BSD one - is explicitly not required to read TZ. So
// declare it and call it whenever TZ changes.
extern "C" {
    fn tzset();
}

/// Read the TZ environment variable into libc's zone state. Call once, before
/// any thread is spawned; `localtime_r` is thread-safe afterwards.
pub fn init_tz() {
    unsafe { tzset() }
}

/// Milliseconds since the epoch, now.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

pub const DAY_MS: i64 = 86_400_000;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// algorithm; the `- 399` keeps it correct under Rust's truncating division).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn local_tm(ms: i64) -> libc::tm {
    let secs = ms.div_euclid(1000) as libc::time_t;
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    }
}

/// Ask libc to resolve a local wall-clock time, DST included. `tm_isdst = -1`
/// makes it work out the offset itself. Out-of-range days roll over, so
/// `d + 1` is a valid way to say "tomorrow".
pub fn local_to_ms(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> Option<i64> {
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = (y - 1900) as libc::c_int;
        tm.tm_mon = (mo - 1) as libc::c_int;
        tm.tm_mday = d as libc::c_int;
        tm.tm_hour = h as libc::c_int;
        tm.tm_min = mi as libc::c_int;
        tm.tm_isdst = -1;
        let t = libc::mktime(&mut tm);
        if t == -1 {
            None
        } else {
            Some(t as i64 * 1000)
        }
    }
}

/// Run `f` with local time meaning the zone `tz` (an IANA name such as
/// `Asia/Colombo`), then restore the previous zone. TZ is process-global, so
/// calls are serialised (which only matters under the parallel test runner) and
/// must not nest.
pub fn with_tz<T>(tz: Option<&str>, f: impl FnOnce() -> T) -> T {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let tz = match tz {
        Some(z) if !z.is_empty() => z,
        _ => return f(),
    };
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old = std::env::var_os("TZ");
    std::env::set_var("TZ", tz);
    init_tz();
    let out = f();
    match old {
        Some(v) => std::env::set_var("TZ", v),
        None => std::env::remove_var("TZ"),
    }
    init_tz();
    out
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Oct` / `october` -> 10.
pub fn month_from_name(s: &str) -> Option<i64> {
    let s = s.to_ascii_lowercase();
    if s.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| s.starts_with(&m.to_ascii_lowercase()))
        .map(|i| i as i64 + 1)
}

/// `Mon Oct 27 3:30pm` in local time, the way /usage writes times.
pub fn fmt_when(ms: i64) -> String {
    let tm = local_tm(ms);
    let h12 = match tm.tm_hour % 12 {
        0 => 12,
        h => h,
    };
    format!(
        "{} {} {} {}:{:02}{}",
        WEEKDAYS[tm.tm_wday.clamp(0, 6) as usize],
        MONTHS[tm.tm_mon.clamp(0, 11) as usize],
        tm.tm_mday,
        h12,
        tm.tm_min,
        if tm.tm_hour < 12 { "am" } else { "pm" }
    )
}

fn num(b: &[u8]) -> Option<i64> {
    if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut n: i64 = 0;
    for c in b {
        n = n.checked_mul(10)?.checked_add((c - b'0') as i64)?;
    }
    Some(n)
}

/// Transcript timestamps to epoch milliseconds: `YYYY-MM-DDTHH:MM:SS[.sss]Z`,
/// or with a `±HH:MM` offset. Anything without an offset is rejected rather than
/// guessed at, since every transcript record carries one.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.trim().as_bytes();
    if b.len() < 17 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let y = num(&b[0..4])?;
    let mo = num(&b[5..7])?;
    let d = num(&b[8..10])?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let rest = &b[11..];
    if rest.len() < 5 || rest[2] != b':' {
        return None;
    }
    let h = num(&rest[0..2])?;
    let mi = num(&rest[3..5])?;
    let mut i = 5;
    let mut sec = 0i64;
    if rest.len() >= i + 3 && rest[i] == b':' {
        sec = num(&rest[i + 1..i + 3])?;
        i += 3;
    }
    let mut frac = 0i64;
    if rest.len() > i && rest[i] == b'.' {
        let start = i + 1;
        let mut end = start;
        while end < rest.len() && rest[end].is_ascii_digit() {
            end += 1;
        }
        // Milliseconds: pad or truncate to exactly three digits.
        let mut digits = [b'0'; 3];
        for (k, slot) in digits.iter_mut().enumerate() {
            if start + k < end {
                *slot = rest[start + k];
            }
        }
        frac = num(&digits)?;
        i = end;
    }
    if h > 24 || mi > 59 || sec > 60 {
        return None;
    }
    let tz = &rest[i..];
    let off_min = if tz == b"Z" || tz == b"z" {
        0
    } else if tz.len() == 6 && (tz[0] == b'+' || tz[0] == b'-') && tz[3] == b':' {
        let sign = if tz[0] == b'-' { -1 } else { 1 };
        sign * (num(&tz[1..3])? * 60 + num(&tz[4..6])?)
    } else {
        return None;
    };
    let days = days_from_civil(y, mo, d);
    Some((days * 86_400 + h * 3600 + mi * 60 + sec - off_min * 60) * 1000 + frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transcript_timestamps() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(parse_iso_ms("1970-01-01T05:30:00+05:30"), Some(0));
        assert_eq!(
            parse_iso_ms("2026-10-21T09:25:10.353Z"),
            Some(1_792_574_710_353)
        );
        assert_eq!(parse_iso_ms("2026-10-21T09:25:10"), None);
        assert_eq!(parse_iso_ms("garbage"), None);
    }

    #[test]
    fn month_names() {
        assert_eq!(month_from_name("Oct"), Some(10));
        assert_eq!(month_from_name("october"), Some(10));
        assert_eq!(month_from_name("Jan"), Some(1));
        assert_eq!(month_from_name("Oc"), None);
        assert_eq!(month_from_name("Foo"), None);
    }
}
