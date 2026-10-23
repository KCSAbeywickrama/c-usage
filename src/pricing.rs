//! API-equivalent cost per request, used as the weight for "share of usage".
//!
//! Raw token totals are a poor weight: most of them are cache reads, which cost
//! a small fraction of an input token, and an Opus token is not a Haiku token.
//! Anthropic does not publish how subscription limits are metered, so list price
//! is the stand-in. The dollar figures are only ever shown as approximate.
//!
//! Source: the claude-api skill's model table (cached 2026-10-06) and
//! prompt-caching notes - cache writes are 1.25x input for the 5-minute TTL and
//! 2x for the 1-hour TTL; cache reads are 0.1x input except where listed.

/// Dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

const fn p(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
    }
}

const TABLE: &[(&str, Price)] = &[
    ("claude-fable-5-1", p(10.0, 50.0, 0.25)),
    ("claude-mythos-5-1", p(10.0, 50.0, 0.25)),
    ("claude-fable-5", p(10.0, 50.0, 1.0)),
    ("claude-opus-5-5", p(4.0, 20.0, 0.20)),
    ("claude-opus-5", p(5.0, 25.0, 0.50)),
    ("claude-opus-4-8", p(5.0, 25.0, 0.50)),
    ("claude-opus-4-7", p(5.0, 25.0, 0.50)),
    ("claude-opus-4-6", p(5.0, 25.0, 0.50)),
    ("claude-sonnet-5-5", p(2.0, 10.0, 0.20)),
    ("claude-sonnet-5", p(2.0, 10.0, 0.20)),
    ("claude-sonnet-4-6", p(3.0, 15.0, 0.30)),
    ("claude-haiku-5-5", p(0.10, 0.50, 0.01)),
    ("claude-haiku-4-5", p(1.0, 5.0, 0.10)),
];

/// Claude Haiku 5.5 is priced higher once the prompt passes 100K tokens.
const HAIKU_5_5_LONG: Price = p(0.50, 2.50, 0.05);
const HAIKU_5_5_LONG_AFTER: u64 = 100_000;

/// Fast mode is 2x standard on the models the reference prices it for.
const FAST_2X: &[&str] = &["claude-opus-5-5", "claude-opus-5"];

#[derive(Clone, Copy, Debug, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    pub fast: bool,
}

impl Tokens {
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_write_5m + self.cache_write_1h + self.cache_read
    }

    pub fn total(&self) -> u64 {
        self.prompt() + self.output
    }
}

/// `claude-opus-5-5[1m]` -> `claude-opus-5-5`. Claude Code tags the 1M-context
/// variant this way in some records; it is priced the same.
pub fn normalize(model: &str) -> &str {
    match model.find('[') {
        Some(i) => &model[..i],
        None => model,
    }
}

/// The table id for a model: an exact match, or an id followed by a dated
/// snapshot suffix (`-20251001`). A longer unknown suffix is not guessed at, so
/// a future `claude-opus-5-7` reports as unpriced instead of as Opus 5.
fn lookup(model: &str) -> Option<(&'static str, Price)> {
    let m = normalize(model);
    TABLE.iter().copied().find(|(id, _)| {
        m == *id
            || m.strip_prefix(id)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|d| d.len() == 8 && d.bytes().all(|c| c.is_ascii_digit()))
    })
}

/// API-equivalent dollars for one request, or None for a model not in the table.
pub fn cost(model: &str, t: &Tokens) -> Option<f64> {
    let (id, mut price) = lookup(model)?;
    if id == "claude-haiku-5-5" && t.prompt() > HAIKU_5_5_LONG_AFTER {
        price = HAIKU_5_5_LONG;
    }
    let mult = if t.fast && FAST_2X.contains(&id) {
        2.0
    } else {
        1.0
    };
    let dollars = t.input as f64 * price.input
        + t.output as f64 * price.output
        + t.cache_write_5m as f64 * price.input * 1.25
        + t.cache_write_1h as f64 * price.input * 2.0
        + t.cache_read as f64 * price.cache_read;
    Some(dollars * mult / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn prices_each_token_kind() {
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            cache_read: 1_000_000,
            fast: false,
        };
        // 4 + 20 + 4*1.25 + 4*2 + 0.20
        assert!(close(cost("claude-opus-5-5", &t).unwrap(), 37.2));
    }

    #[test]
    fn fast_mode_doubles_opus_only() {
        let t = Tokens {
            output: 1_000_000,
            fast: true,
            ..Default::default()
        };
        assert!(close(cost("claude-opus-5-5", &t).unwrap(), 40.0));
        assert!(close(cost("claude-sonnet-5-5", &t).unwrap(), 10.0));
    }

    #[test]
    fn haiku_long_prompt_tier() {
        let short = Tokens {
            input: 100_000,
            ..Default::default()
        };
        let long = Tokens {
            input: 100_001,
            ..Default::default()
        };
        assert!(close(cost("claude-haiku-5-5", &short).unwrap(), 0.01));
        assert!(close(
            cost("claude-haiku-5-5", &long).unwrap(),
            100_001.0 * 0.5 / 1e6
        ));
    }

    #[test]
    fn model_name_variants() {
        assert!(lookup("claude-opus-5-5[1m]").is_some());
        assert!(lookup("claude-haiku-4-5-20251001").is_some());
        // Longest match is not a prefix match: these must not fall back to Opus 5.
        assert_eq!(lookup("claude-opus-5-5").unwrap().0, "claude-opus-5-5");
        assert!(lookup("claude-opus-5-7").is_none());
        assert!(lookup("<synthetic>").is_none());
    }
}
