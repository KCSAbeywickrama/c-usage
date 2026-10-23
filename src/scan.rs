//! Every API response recorded in the local transcripts since a cutoff.
//!
//! Main sessions sit at `<project>/<sessionId>.jsonl` and subagents at
//! `<project>/<sessionId>/subagents/agent-<id>.jsonl`. Each assistant record
//! carries the response's `usage`, the model, a timestamp and the `cwd`.
//!
//! The same response appears many times: Claude Code writes one record per
//! content block, each repeating the full `usage`, and a resumed or forked
//! session copies earlier records into its new file. So responses are counted
//! once per `message.id` (or `requestId`), across all files.

use crate::pricing::Tokens;
use crate::scope::Scope;
use memchr::memmem;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One API response.
#[derive(Debug, Clone)]
pub struct Req {
    pub ts: i64,
    pub model: String,
    pub tokens: Tokens,
    pub in_project: bool,
}

#[derive(Deserialize)]
struct Rec {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<Msg>,
}

#[derive(Deserialize)]
struct Msg {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
    speed: Option<String>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

impl Usage {
    fn tokens(&self) -> Tokens {
        let written = self.cache_creation_input_tokens.unwrap_or(0);
        let (w5, w1h) = match &self.cache_creation {
            Some(c) => {
                let w5 = c.ephemeral_5m_input_tokens.unwrap_or(0);
                let w1h = c.ephemeral_1h_input_tokens.unwrap_or(0);
                // Anything the breakdown does not account for is priced as the
                // cheaper 5-minute write.
                (w5 + written.saturating_sub(w5 + w1h), w1h)
            }
            None => (written, 0),
        };
        Tokens {
            input: self.input_tokens.unwrap_or(0),
            output: self.output_tokens.unwrap_or(0),
            cache_write_5m: w5,
            cache_write_1h: w1h,
            cache_read: self.cache_read_input_tokens.unwrap_or(0),
            fast: self.speed.as_deref() == Some("fast"),
        }
    }
}

pub fn projects_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("CLAUDE_PROJECTS_DIR") {
        return PathBuf::from(v);
    }
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(v) => PathBuf::from(v),
        None => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".claude")
        }
    };
    base.join("projects")
}

fn modified_ms(path: &Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = m.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_millis() as i64)
}

fn jsonl_in(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Transcript files that could hold a record at or after `since_ms`. A file
/// last modified before the cutoff cannot, since transcripts are append-only.
pub fn collect_files(root: &Path, since_ms: i64) -> Result<Vec<PathBuf>, String> {
    let projects = std::fs::read_dir(root).map_err(|e| {
        format!(
            "cannot read {}: {e}\n       set CLAUDE_PROJECTS_DIR if your transcripts live elsewhere.",
            root.display()
        )
    })?;
    let mut out = Vec::new();
    for proj in projects.filter_map(|e| e.ok()) {
        if !proj.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let proj = proj.path();
        out.extend(jsonl_in(&proj));
        if let Ok(entries) = std::fs::read_dir(&proj) {
            for session in entries.filter_map(|e| e.ok()) {
                if session.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    out.extend(jsonl_in(&session.path().join("subagents")));
                }
            }
        }
    }
    out.retain(|p| modified_ms(p).is_some_and(|m| m >= since_ms));
    out.sort();
    Ok(out)
}

enum Data {
    Map(memmap2::Mmap),
    Buf(Vec<u8>),
}

impl std::ops::Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Map(m) => m,
            Data::Buf(b) => b,
        }
    }
}

fn read_file(path: &Path) -> std::io::Result<Data> {
    let f = std::fs::File::open(path)?;
    if f.metadata()?.len() == 0 {
        return Ok(Data::Buf(Vec::new()));
    }
    // SAFETY: transcripts are append-only, so the mapping cannot be shortened
    // underneath us. Any mapping failure falls back to a plain read.
    match unsafe { memmap2::Mmap::map(&f) } {
        Ok(m) => Ok(Data::Map(m)),
        Err(_) => Ok(Data::Buf(std::fs::read(path)?)),
    }
}

/// Every distinct response at or after `since_ms`, with `in_project` set by
/// whether its recorded `cwd` is at or below the scope.
pub fn collect(files: &[PathBuf], scope: &Scope, since_ms: i64) -> Vec<Req> {
    // Cheap gates before any JSON parsing: most lines are user turns, tool
    // results and metadata, and none of those carry "usage".
    let has_usage = memmem::Finder::new(b"\"usage\"");
    let has_assistant = memmem::Finder::new(b"\"assistant\"");

    let mut reqs: Vec<Req> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    for path in files {
        let data = match read_file(path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for line in data.split(|&b| b == b'\n') {
            if has_usage.find(line).is_none() || has_assistant.find(line).is_none() {
                continue;
            }
            let rec: Rec = match serde_json::from_slice(line) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if rec.kind.as_deref() != Some("assistant") {
                continue;
            }
            let Some(msg) = rec.message else { continue };
            let (Some(usage), Some(model)) = (msg.usage, msg.model) else {
                continue;
            };
            if model.starts_with('<') {
                // `<synthetic>`: locally generated, never sent to the API.
                continue;
            }
            let Some(ts) = rec.timestamp.as_deref().and_then(crate::time::parse_iso_ms) else {
                continue;
            };
            if ts < since_ms {
                continue;
            }
            let req = Req {
                ts,
                model,
                tokens: usage.tokens(),
                in_project: rec.cwd.as_deref().is_some_and(|c| scope.contains(c)),
            };
            match msg.id.or(rec.request_id) {
                Some(key) => match seen.get(&key) {
                    // A repeat. They normally carry identical usage; if a
                    // streamed copy was written before the final counts, keep
                    // the larger one.
                    Some(&i) => {
                        if req.tokens.output > reqs[i].tokens.output {
                            reqs[i].tokens = req.tokens;
                        }
                    }
                    None => {
                        seen.insert(key, reqs.len());
                        reqs.push(req);
                    }
                },
                None => reqs.push(req),
            }
        }
    }
    reqs
}
