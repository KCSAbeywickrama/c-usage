//! Colour decision. Adapted from claude-find's term.rs: `isatty` and SGR
//! escapes, nothing more.

use std::io::IsTerminal;

const ESC: char = '\u{1b}';

pub struct Colors {
    pub on: bool,
}

/// An unset *or* empty variable counts as off.
fn env_truthy(key: &str) -> bool {
    std::env::var_os(key)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

impl Colors {
    /// Precedence: explicit flag, NO_COLOR, FORCE_COLOR, CLAUDECODE (so
    /// `!cusage` inside Claude Code stays coloured), then whether
    /// stdout is a terminal.
    pub fn new(pref: Option<bool>) -> Self {
        let on = match pref {
            Some(v) => v,
            None => {
                if env_truthy("NO_COLOR") {
                    false
                } else if env_truthy("FORCE_COLOR") || env_truthy("CLAUDECODE") {
                    true
                } else {
                    std::io::stdout().is_terminal()
                }
            }
        };
        Colors { on }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("{ESC}[{code}m{s}{ESC}[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
}
