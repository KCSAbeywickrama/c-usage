//! End to end: a fixture transcripts tree, a fake `claude` that prints /usage
//! output, and the real binary run from inside the fixture project.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const BIN: &str = env!("CARGO_BIN_EXE_c-usage");

/// Inverse of days_from_civil (Howard Hinnant), for writing UTC timestamps.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn iso(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn record(id: &str, model: &str, output: u64, cwd: &Path, ts: i64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{}","cwd":"{}","requestId":"req_{id}","message":{{"id":"msg_{id}","model":"{model}","role":"assistant","content":[{{"type":"text","text":"hi"}}],"usage":{{"input_tokens":0,"output_tokens":{output},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#,
        iso(ts),
        cwd.display()
    ) + "\n"
}

struct Fixture {
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture(name: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "cusage-test-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    Fixture {
        root: fs::canonicalize(&root).unwrap(),
    }
}

fn fake_claude(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("claude");
    fs::write(&path, format!("#!/bin/sh\ncat <<'OUT'\n{body}\nOUT\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn run(fx: &Fixture, claude: &Path) -> (String, i32) {
    let out = Command::new(BIN)
        .current_dir(fx.root.join("proj"))
        .env("CLAUDE_PROJECTS_DIR", fx.root.join("projects"))
        .env("CUSAGE_CLAUDE_BIN", claude)
        .env("PWD", fx.root.join("proj"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// The project at `<root>/proj`, a sibling at `<root>/proj-other`, and their
/// transcripts. API-equivalent cost by design (output tokens only):
///   in project, in window:  msg a $20 (opus-5-5, repeated and forked), msg b $20 (subagent, subfolder)
///   sibling, in window:     msg c $40 (opus-5-5), msg e $50 (fable-5-1)
///   in project, too old:    msg d $100
/// So the share is 40 / 130 = 30.8%, and 0% of Fable.
fn build(fx: &Fixture) {
    let proj = fx.root.join("proj");
    let other = fx.root.join("proj-other");
    fs::create_dir_all(proj.join("sub")).unwrap();
    fs::create_dir_all(&other).unwrap();
    let pdir = fx.root.join("projects").join("-proj");
    let odir = fx.root.join("projects").join("-proj-other");
    fs::create_dir_all(pdir.join("s1").join("subagents")).unwrap();
    fs::create_dir_all(&odir).unwrap();

    let now = now_secs();
    let recent = now - 3600;
    let a = record("a", "claude-opus-5-5", 1_000_000, &proj, recent);
    // One record per content block, all repeating the same usage.
    let s1 = format!(
        "{a}{a}{a}{}{}",
        r#"{"type":"user","message":{"role":"user","content":"hello"}}"#.to_string() + "\n",
        record("d", "claude-opus-5-5", 5_000_000, &proj, now - 9 * 86_400)
    );
    fs::write(pdir.join("s1.jsonl"), s1).unwrap();
    // A fork copies the history into a new file.
    fs::write(pdir.join("s2.jsonl"), &a).unwrap();
    fs::write(
        pdir.join("s1").join("subagents").join("agent-x.jsonl"),
        record(
            "b",
            "claude-opus-5-5[1m]",
            1_000_000,
            &proj.join("sub"),
            recent,
        ),
    )
    .unwrap();
    fs::write(
        odir.join("s3.jsonl"),
        record("c", "claude-opus-5-5", 2_000_000, &other, recent)
            + &record("e", "claude-fable-5-1", 1_000_000, &other, recent)
            + &record("syn", "<synthetic>", 9_000_000, &proj, recent),
    )
    .unwrap();
}

#[test]
fn splits_the_weekly_limit_by_project() {
    let fx = fixture("split");
    build(&fx);
    // Time-only reset: the next 11:59pm UTC, so "now - 1h" is always inside
    // the week and "now - 9 days" never is.
    let claude = fake_claude(
        &fx.root,
        "Current session: 10% used · resets 11:59pm (UTC)\n\
         Current week (all models): 44% used · resets 11:59pm (UTC)\n\
         Current week (Fable): 72% used · resets 11:59pm (UTC)",
    );
    let (out, code) = run(&fx, &claude);
    assert_eq!(code, 0, "{out}");
    let line = |prefix: &str| {
        out.lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` line in:\n{out}"))
            .split_whitespace()
            .collect::<Vec<_>>()
    };
    // 40/130 of 44% is 13.5%.
    assert_eq!(line("all models"), ["all", "models", "44%", "~14%", "31%"]);
    assert_eq!(line("Fable"), ["Fable", "72%", "~0.0%", "0.0%"]);
    assert_eq!(line("this project"), ["this", "project", "2.0M", "tokens"]);
    assert_eq!(line("all projects"), ["all", "projects", "5.0M", "tokens"]);
}

#[test]
fn falls_back_when_usage_is_unreadable() {
    let fx = fixture("fallback");
    build(&fx);
    let claude = fake_claude(&fx.root, "Unknown skill: usage");
    let (out, code) = run(&fx, &claude);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("couldn't read the weekly limit from /usage: no weekly limit in its output (\"Unknown skill: usage\")"), "{out}");
    assert!(out.contains("share of local use   31%"), "{out}");
    assert!(!out.contains("~ = share"), "{out}");
}
