//! The account-wide weekly limits, read from Claude Code's own `/usage`.
//!
//! `claude -p --no-session-persistence "/usage"` runs the slash command without
//! a model call (so it spends no usage) and without writing a transcript. Its
//! output is meant for people, not programs:
//!
//! ```text
//! Current session: 23% used · resets Oct 23 at 7:20pm (Asia/Colombo)
//! Current week (all models): 44% used · resets Oct 27 at 3:30pm (Asia/Colombo)
//! Current week (Fable): 72% used · resets Oct 27 at 3:30pm (Asia/Colombo)
//! ```
//!
//! so the parser is tolerant, and anything it cannot read is reported as a
//! reason for the caller to fall back on, never as a hard failure.

use crate::time;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct WeekLimit {
    /// What /usage puts in the parentheses: `all models`, `Fable`, ...
    pub scope: String,
    pub pct: f64,
    pub reset_ms: i64,
}

/// Which `claude` to run. Overridable so tests can stand in a fake.
fn claude_bin() -> String {
    std::env::var("CLAUDE_TOKEN_USAGE_CLAUDE_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "claude".to_string())
}

/// Run `/usage` and return its stdout.
pub fn run(timeout: Duration) -> Result<String, String> {
    let bin = claude_bin();
    let mut child = Command::new(&bin)
        .args(["-p", "--no-session-persistence", "/usage"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `{bin}`: {e}"))?;

    // Drain both pipes on their own threads so a chatty child cannot block on a
    // full pipe while we wait for it to exit.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`{bin} -p /usage` timed out after {}s",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("waiting for `{bin}`: {e}")),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if !status.success() {
        let why = err
            .lines()
            .chain(out.lines())
            .find(|l| !l.trim().is_empty());
        return Err(format!(
            "`{bin} -p /usage` failed ({status}){}",
            why.map(|w| format!(": {}", w.trim())).unwrap_or_default()
        ));
    }
    Ok(out)
}

/// Every `Current week (...)` line that parses. The session (5h) line is ignored.
/// Must be called while the process is single-threaded (see `time::with_tz`).
pub fn parse(text: &str, now_ms: i64) -> Vec<WeekLimit> {
    text.lines()
        .filter_map(|l| parse_week_line(l.trim(), now_ms))
        .collect()
}

fn parse_week_line(line: &str, now_ms: i64) -> Option<WeekLimit> {
    let rest = line.strip_prefix("Current week (")?;
    let close = rest.find("):")?;
    let scope = rest[..close].trim().to_string();
    let after = rest[close + 2..].trim();
    let pct: f64 = after[..after.find('%')?].trim().parse().ok()?;
    // ASCII lowercasing keeps byte offsets, so the index is valid in `after`.
    let at = after.to_ascii_lowercase().find("resets ")?;
    let reset_ms = parse_reset(after[at + "resets ".len()..].trim(), now_ms)?;
    Some(WeekLimit {
        scope,
        pct,
        reset_ms,
    })
}

/// Only plain IANA names reach TZ: `Asia/Colombo`, `America/Argentina/Buenos_Aires`,
/// `UTC`. Anything else, or a name this machine has no zone file for, falls back
/// to local time rather than to libc's silent UTC.
fn usable_tz(tz: &str) -> Option<&str> {
    let plain = !tz.is_empty()
        && !tz.contains("..")
        && tz
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'+'));
    if !plain {
        return None;
    }
    let dir = std::env::var("TZDIR").unwrap_or_else(|_| "/usr/share/zoneinfo".to_string());
    std::path::Path::new(&dir).join(tz).is_file().then_some(tz)
}

/// `Oct 27 at 3:30pm (Asia/Colombo)`, `Oct 27, 2026 at 3pm`, or a bare
/// `3:30pm (Asia/Colombo)` when the reset is close enough that /usage drops the
/// date. The result is the next such moment, never one more than a day past.
fn parse_reset(s: &str, now_ms: i64) -> Option<i64> {
    let (when, tz) = match (s.rfind('('), s.ends_with(')')) {
        (Some(open), true) => (s[..open].trim(), usable_tz(s[open + 1..s.len() - 1].trim())),
        _ => (s, None),
    };
    let (date, clock) = match when.find(" at ") {
        Some(i) => (Some(when[..i].trim()), when[i + 4..].trim()),
        None => (None, when),
    };
    let (h, mi) = parse_clock(clock)?;
    let date = match date {
        Some(d) => Some(parse_date(d)?),
        None => None,
    };

    time::with_tz(tz, || {
        let now = time::local_tm(now_ms);
        let this_year = now.tm_year as i64 + 1900;
        match date {
            Some((mo, d, Some(y))) => time::local_to_ms(y, mo, d, h, mi),
            Some((mo, d, None)) => {
                let ts = time::local_to_ms(this_year, mo, d, h, mi)?;
                // "Jan 2" read on Dec 30 means next year.
                if ts < now_ms - time::DAY_MS {
                    time::local_to_ms(this_year + 1, mo, d, h, mi)
                } else {
                    Some(ts)
                }
            }
            None => {
                let (mo, d) = (now.tm_mon as i64 + 1, now.tm_mday as i64);
                let ts = time::local_to_ms(this_year, mo, d, h, mi)?;
                if ts < now_ms {
                    time::local_to_ms(this_year, mo, d + 1, h, mi)
                } else {
                    Some(ts)
                }
            }
        }
    })
}

/// `3:30pm`, `3pm`, `3:30 PM`, `15:30` -> (15, 30).
fn parse_clock(s: &str) -> Option<(i64, i64)> {
    let s = s.trim().to_ascii_lowercase();
    let (digits, meridiem) = if let Some(d) = s.strip_suffix("am") {
        (d.trim(), Some(false))
    } else if let Some(d) = s.strip_suffix("pm") {
        (d.trim(), Some(true))
    } else {
        (s.as_str(), None)
    };
    let (h, mi) = match digits.split_once(':') {
        Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
        None => (digits.parse::<i64>().ok()?, 0),
    };
    if !(0..60).contains(&mi) {
        return None;
    }
    let h = match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            (h % 12) + if pm { 12 } else { 0 }
        }
        None if (0..24).contains(&h) => h,
        None => return None,
    };
    Some((h, mi))
}

/// `Oct 27` or `Oct 27, 2026` -> (10, 27, year?).
fn parse_date(s: &str) -> Option<(i64, i64, Option<i64>)> {
    let mut parts = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty());
    let mo = time::month_from_name(parts.next()?)?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=31).contains(&d) {
        return None;
    }
    let y = match parts.next() {
        Some(y) => Some(y.parse::<i64>().ok()?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((mo, d, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests read times in UTC so they do not depend on the machine's zone.
    const REAL: &str = "You are currently using your subscription to power your Claude Code usage

Current session: 23% used · resets Oct 23 at 7:20pm (Asia/Colombo)
Current week (all models): 44% used · resets Oct 27 at 3:30pm (UTC)
Current week (Fable): 72% used · resets Oct 27 at 3:30pm (UTC)

What's contributing to your limits usage?
";

    fn utc(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> i64 {
        time::with_tz(Some("UTC"), || time::local_to_ms(y, mo, d, h, mi).unwrap())
    }

    #[test]
    fn parses_real_output() {
        let now = utc(2026, 10, 23, 16, 0);
        let got = parse(REAL, now);
        let reset = utc(2026, 10, 27, 15, 30);
        assert_eq!(
            got,
            vec![
                WeekLimit {
                    scope: "all models".into(),
                    pct: 44.0,
                    reset_ms: reset
                },
                WeekLimit {
                    scope: "Fable".into(),
                    pct: 72.0,
                    reset_ms: reset
                },
            ]
        );
    }

    #[test]
    fn reads_the_named_zone() {
        // 3:30pm in Colombo (+05:30) is 10:00 UTC.
        let now = utc(2026, 10, 23, 16, 0);
        let got = parse_reset("Oct 27 at 3:30pm (Asia/Colombo)", now).unwrap();
        assert_eq!(got, utc(2026, 10, 27, 10, 0));
    }

    #[test]
    fn year_rolls_over() {
        let now = utc(2026, 12, 30, 12, 0);
        assert_eq!(
            parse_reset("Jan 2 at 9am (UTC)", now),
            Some(utc(2027, 1, 2, 9, 0))
        );
        assert_eq!(
            parse_reset("Jan 2, 2027 at 9am (UTC)", now),
            Some(utc(2027, 1, 2, 9, 0))
        );
    }

    #[test]
    fn time_only_means_the_next_occurrence() {
        let now = utc(2026, 10, 23, 16, 0);
        assert_eq!(
            parse_reset("7:20pm (UTC)", now),
            Some(utc(2026, 10, 23, 19, 20))
        );
        assert_eq!(parse_reset("9am (UTC)", now), Some(utc(2026, 10, 24, 9, 0)));
    }

    #[test]
    fn clock_formats() {
        assert_eq!(parse_clock("3:30pm"), Some((15, 30)));
        assert_eq!(parse_clock("3 PM"), Some((15, 0)));
        assert_eq!(parse_clock("12am"), Some((0, 0)));
        assert_eq!(parse_clock("12:05pm"), Some((12, 5)));
        assert_eq!(parse_clock("15:30"), Some((15, 30)));
        assert_eq!(parse_clock("13pm"), None);
        assert_eq!(parse_clock("noon"), None);
    }

    #[test]
    fn unreadable_lines_are_skipped() {
        let now = utc(2026, 10, 23, 16, 0);
        assert!(parse("Current week (all models): lots used", now).is_empty());
        assert!(parse("Current week (all models): 44% used · resets soon", now).is_empty());
        assert!(parse("", now).is_empty());
    }

    #[test]
    fn unsafe_or_unknown_zone_names_are_ignored() {
        assert_eq!(usable_tz("../../etc/passwd"), None);
        assert_eq!(usable_tz("Not/AZone"), None);
        assert_eq!(usable_tz("UTC"), Some("UTC"));
    }
}
