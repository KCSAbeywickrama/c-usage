//! Totals for the window, and the text the user sees.

use crate::pricing::{self, Tokens};
use crate::scan::Req;
use crate::term::Colors;
use crate::time;
use crate::usage::WeekLimit;
use std::collections::BTreeMap;

#[derive(Default, Clone, Copy)]
struct Sum {
    tokens: u64,
    cost: f64,
}

impl Sum {
    fn add(&mut self, t: &Tokens, cost: f64) {
        self.tokens += t.total();
        self.cost += cost;
    }
}

/// Per-model totals for one window, keyed by the normalised model id.
#[derive(Default)]
pub struct Totals {
    project: BTreeMap<String, Sum>,
    all: BTreeMap<String, Sum>,
    /// Tokens from models with no price, which are left out of every share.
    unpriced: BTreeMap<String, u64>,
}

impl Totals {
    pub fn new(reqs: &[Req], start_ms: i64, end_ms: i64) -> Self {
        let mut t = Totals::default();
        for r in reqs.iter().filter(|r| r.ts >= start_ms && r.ts < end_ms) {
            let model = pricing::normalize(&r.model).to_string();
            let Some(cost) = pricing::cost(&r.model, &r.tokens) else {
                *t.unpriced.entry(model).or_default() += r.tokens.total();
                continue;
            };
            t.all.entry(model.clone()).or_default().add(&r.tokens, cost);
            if r.in_project {
                t.project.entry(model).or_default().add(&r.tokens, cost);
            }
        }
        t
    }

    /// This project's share of local cost over the models a limit covers, or
    /// None when there was no local usage of those models at all.
    pub fn share(&self, covers: Option<&str>) -> Option<f64> {
        // fold from +0.0: an empty f64 `sum()` is -0.0, which prints as "-0.0%".
        let sum = |m: &BTreeMap<String, Sum>| -> f64 {
            m.iter()
                .filter(|(model, _)| covers.map_or(true, |w| model.contains(w)))
                .fold(0.0, |acc, (_, s)| acc + s.cost)
        };
        let all = sum(&self.all);
        (all > 0.0).then(|| sum(&self.project) / all)
    }

    fn total(m: &BTreeMap<String, Sum>) -> Sum {
        m.values().fold(Sum::default(), |mut acc, s| {
            acc.tokens += s.tokens;
            acc.cost += s.cost;
            acc
        })
    }
}

/// Which models a `/usage` limit covers: `all models` is everything; `Fable` or
/// `Sonnet only` is the models whose id contains that family name.
pub fn covers(scope: &str) -> Option<String> {
    let s = scope.trim().to_ascii_lowercase();
    if s == "all models" || s.is_empty() {
        return None;
    }
    s.split_whitespace().next().map(str::to_string)
}

fn human_tokens(n: u64) -> String {
    let f = n as f64;
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}K", f / 1e3)
    } else if n < 1_000_000_000 {
        format!("{:.1}M", f / 1e6)
    } else {
        format!("{:.2}B", f / 1e9)
    }
}

/// One decimal below 10%, where the decimal still carries information.
fn pct(x: f64) -> String {
    if x < 10.0 {
        format!("{x:.1}%")
    } else {
        format!("{x:.0}%")
    }
}

fn short_model(m: &str) -> &str {
    m.strip_prefix("claude-").unwrap_or(m)
}

/// Left-align the first column, right-align the rest. Colour is applied after
/// padding so escape codes do not upset the widths.
fn table(rows: &[Vec<String>], c: &Colors, header: bool) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|i| {
            rows.iter()
                .filter_map(|r| r.get(i))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for (n, row) in rows.iter().enumerate() {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            if i == 0 {
                line.push_str(cell);
                line.push_str(&" ".repeat(pad));
            } else {
                line.push_str("   ");
                line.push_str(&" ".repeat(pad));
                line.push_str(cell);
            }
        }
        let line = line.trim_end();
        out.push_str(&if header && n == 0 {
            c.dim(line)
        } else {
            line.to_string()
        });
        out.push('\n');
    }
    out
}

pub struct View<'a> {
    pub label: &'a str,
    pub start_ms: i64,
    pub end_ms: i64,
    /// The weekly limits from /usage, or why they could not be read.
    pub limits: Result<&'a [WeekLimit], &'a str>,
    pub totals: &'a Totals,
}

pub fn render(v: &View, c: &Colors) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}  {}\n", c.bold("claude-token-usage"), v.label));

    match v.limits {
        Ok(limits) => {
            out.push_str(&format!(
                "{}  {} → {}\n\n",
                c.dim("week"),
                time::fmt_when(v.start_ms),
                time::fmt_when(v.end_ms)
            ));
            let mut rows = vec![vec![
                String::new(),
                "account".to_string(),
                "this project".to_string(),
                "share of local use".to_string(),
            ]];
            for l in limits {
                let share = v.totals.share(covers(&l.scope).as_deref());
                rows.push(vec![
                    l.scope.clone(),
                    pct(l.pct),
                    share.map_or("-".to_string(), |s| format!("~{}", pct(s * l.pct))),
                    share.map_or("-".to_string(), |s| pct(s * 100.0)),
                ]);
            }
            out.push_str(&table(&rows, c, true));
        }
        Err(why) => {
            out.push_str(&format!(
                "{}  last 7 days, {} → {}\n",
                c.dim("week"),
                time::fmt_when(v.start_ms),
                time::fmt_when(v.end_ms)
            ));
            out.push_str(&c.yellow(&format!(
                "couldn't read the weekly limit from /usage: {why}\n"
            )));
            out.push('\n');
            let share = v.totals.share(None);
            out.push_str(&format!(
                "share of local use   {}\n",
                share.map_or("-".to_string(), |s| pct(s * 100.0))
            ));
        }
    }

    out.push('\n');
    let mut rows = Vec::new();
    let proj = Totals::total(&v.totals.project);
    rows.push(vec![
        "this project".to_string(),
        format!("{} tokens", human_tokens(proj.tokens)),
    ]);
    for (model, s) in &v.totals.project {
        rows.push(vec![
            format!("  {}", short_model(model)),
            human_tokens(s.tokens),
        ]);
    }
    let all = Totals::total(&v.totals.all);
    rows.push(vec![
        "all projects".to_string(),
        format!("{} tokens", human_tokens(all.tokens)),
    ]);
    out.push_str(&table(&rows, c, false));

    if !v.totals.unpriced.is_empty() {
        let list: Vec<String> = v
            .totals
            .unpriced
            .iter()
            .map(|(m, n)| format!("{} {}", short_model(m), human_tokens(*n)))
            .collect();
        out.push_str(&c.yellow(&format!(
            "\nno price for: {} (left out of the share)\n",
            list.join(", ")
        )));
    }

    out.push('\n');
    // Cost is only a weight here. Shown as dollars it reads like a bill, which
    // it is not on a subscription, so it stays internal.
    out.push_str(&c.dim(
        "share = this project's part of this week's local Claude Code usage, weighted\n\
         by model price (a Fable token counts more than a Haiku one; cache reads count little).",
    ));
    out.push('\n');
    if v.limits.is_ok() {
        out.push_str(&c.dim(
            "~ = share × account %. Usage from claude.ai or other machines is not in the\n\
             local logs; if you have any, this project's real % is lower.",
        ));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(model: &str, output: u64, in_project: bool, ts: i64) -> Req {
        Req {
            ts,
            model: model.to_string(),
            tokens: Tokens {
                output,
                ..Default::default()
            },
            in_project,
        }
    }

    #[test]
    fn share_is_cost_weighted_and_windowed() {
        let reqs = vec![
            // $20 in project
            req("claude-opus-5-5", 1_000_000, true, 10),
            // $50 elsewhere, Fable
            req("claude-fable-5-1", 1_000_000, false, 20),
            // $10 in project, outside the window
            req("claude-sonnet-5-5", 1_000_000, true, 99),
            // unpriced: ignored
            req("claude-future-9", 1_000_000, true, 30),
        ];
        let t = Totals::new(&reqs, 0, 50);
        assert!((t.share(None).unwrap() - 20.0 / 70.0).abs() < 1e-9);
        assert_eq!(t.share(Some("fable")), Some(0.0));
        assert_eq!(t.share(Some("sonnet")), None);
        assert_eq!(t.unpriced.get("claude-future-9"), Some(&1_000_000));
    }

    #[test]
    fn limit_scopes() {
        assert_eq!(covers("all models"), None);
        assert_eq!(covers("Fable"), Some("fable".to_string()));
        assert_eq!(covers("Sonnet only"), Some("sonnet".to_string()));
    }

    #[test]
    fn number_formats() {
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(8_400_000), "8.4M");
        assert_eq!(human_tokens(1_230_000_000), "1.23B");
        assert_eq!(pct(5.66), "5.7%");
        assert_eq!(pct(44.0), "44%");
    }
}
