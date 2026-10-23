//! c-usage - how much of your weekly Claude limit the project you
//! are standing in has used.
//!
//! Anthropic reports the weekly limit for the whole account only. This reads
//! that from Claude Code's own `/usage` (no model call, no usage spent), adds up
//! the local transcripts for the same week, and splits the account % by this
//! project's share of local API-equivalent cost.

mod pricing;
mod report;
mod scan;
mod scope;
mod term;
mod time;
mod usage;

use std::io::Write;
use std::time::Duration;

const HELP: &str = "\
c-usage - this project's share of your weekly Claude limit

usage: c-usage [--no-color | --color]
       cusage [--no-color | --color]

Run it inside a project folder. It reads the weekly limit from
`claude -p --no-session-persistence /usage` (no model call, no usage spent)
and the matching week of local transcripts under ~/.claude/projects, then
shows this project's share of local Claude Code usage and the estimated %
of the weekly limit that represents.

Sessions started in this folder or any folder below it count as the project.

options:
  --no-color   plain output
  --color      colour even when piped
  -h, --help   this text
  -V, --version

environment:
  CLAUDE_PROJECTS_DIR   transcripts root (default ~/.claude/projects)
  CUSAGE_CLAUDE_BIN     the claude executable to run (default claude)
  NO_COLOR, FORCE_COLOR
";

/// Long enough for a cold start of `claude`; /usage normally answers in ~4s.
const USAGE_TIMEOUT: Duration = Duration::from_secs(30);

fn fail(msg: &str) -> ! {
    eprintln!("c-usage: {msg}");
    std::process::exit(2);
}

fn emit(s: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match lock.write_all(s.as_bytes()).and_then(|_| lock.flush()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => fail(&format!("cannot write to stdout: {e}")),
    }
}

fn main() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    time::init_tz();

    let mut color = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--no-color" => color = Some(false),
            "--color" => color = Some(true),
            "-h" | "--help" => {
                emit(HELP);
                return;
            }
            "-V" | "--version" => {
                emit(&format!("c-usage {}\n", env!("CARGO_PKG_VERSION")));
                return;
            }
            other => fail(&format!("unknown argument: {other}\n       try --help")),
        }
    }
    let colors = term::Colors::new(color);
    let scope = scope::Scope::here();
    let now = time::now_ms();

    // /usage takes a few seconds, so read the transcripts while it runs. A week
    // ends at its reset, which is never more than 7 days away, so 8 days back is
    // a safe superset of whatever window /usage reports.
    let fetch = std::thread::spawn(|| usage::run(USAGE_TIMEOUT));
    let since = now - 8 * time::DAY_MS;
    let root = scan::projects_dir();
    let files = scan::collect_files(&root, since).unwrap_or_else(|e| fail(&e));
    let reqs = scan::collect(&files, &scope, since);
    let fetched = fetch
        .join()
        .unwrap_or_else(|_| Err("reading /usage panicked".to_string()));

    // Every other thread has finished: parsing may now switch TZ.
    let limits = fetched.and_then(|text| {
        let limits = usage::parse(&text, now);
        if limits.is_empty() {
            let first = text.lines().map(str::trim).find(|l| !l.is_empty());
            Err(format!(
                "no weekly limit in its output{}",
                first.map(|l| format!(" (\"{l}\")")).unwrap_or_default()
            ))
        } else {
            Ok(limits)
        }
    });

    let (start, end) = match &limits {
        Ok(l) => {
            let all = l
                .iter()
                .find(|w| report::covers(&w.scope).is_none())
                .unwrap_or(&l[0]);
            (all.reset_ms - 7 * time::DAY_MS, all.reset_ms)
        }
        Err(_) => (now - 7 * time::DAY_MS, now),
    };

    let totals = report::Totals::new(&reqs, start, end.max(now));
    let home = scope::home_dir();
    let label = scope.label(home.as_deref());
    let view = report::View {
        label: &label,
        start_ms: start,
        end_ms: end,
        limits: limits.as_deref().map_err(String::as_str),
        totals: &totals,
    };
    emit(&report::render(&view, &colors));
}
