//! Which transcript records belong to the directory you ran the tool from.
//!
//! Adapted from claude-find's scope.rs. Deriving a project directory name from
//! the cwd does not work: the `~/.claude/projects` mangling is lossy, and
//! sessions are filed under the directory they *started* in, so a session that
//! ran in `repo/services/api` may live under the repo root's directory. The
//! authoritative test is the `cwd` recorded on each transcript record, which is
//! what `contains` checks.
//!
//! Every project directory is read anyway (the share needs the all-projects
//! total), so claude-find's directory-name prefilter is not needed here.

use std::path::{Path, PathBuf};

pub struct Scope {
    /// Every path that names the current directory - see `cwd_aliases`. Empty
    /// means global: no filtering at all.
    cwds: Vec<PathBuf>,
}

/// Is `cand` the same directory as `base`, or below it? Compared by path
/// component, so `/a/bc` is not "below" `/a/b`.
fn at_or_below(base: &Path, cand: &Path) -> bool {
    let mut b = base.components();
    let mut c = cand.components();
    loop {
        match b.next() {
            None => return true,
            Some(want) => match c.next() {
                Some(got) if got == want => continue,
                _ => return false,
            },
        }
    }
}

/// Every path that names the current directory: what getcwd reports, the
/// symlink-resolved form when it differs, and $PWD when it is a genuine alias
/// for the same directory. On macOS these routinely disagree - /var is a
/// symlink to /private/var - and a transcript may have recorded any of them.
/// Empty when the cwd cannot be determined at all.
fn cwd_aliases() -> Vec<PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let mut cwds = vec![cwd.clone()];
    let real = std::fs::canonicalize(&cwd).ok();
    if let Some(r) = &real {
        if !cwds.contains(r) {
            cwds.push(r.clone());
        }
    }
    // $PWD only counts if it really is this directory: a child process can
    // inherit a stale PWD from its parent.
    if let Some(pwd) = std::env::var_os("PWD") {
        let pwd = PathBuf::from(pwd);
        let same = std::fs::canonicalize(&pwd).ok() == real && real.is_some();
        if same && !cwds.contains(&pwd) {
            cwds.push(pwd);
        }
    }
    cwds
}

impl Scope {
    pub fn global() -> Self {
        Scope { cwds: Vec::new() }
    }

    /// Scope to the current directory. Falls back to global if the cwd cannot be
    /// determined at all, which is better than silently finding nothing.
    pub fn here() -> Self {
        let cwds = cwd_aliases();
        if cwds.is_empty() {
            return Scope::global();
        }
        Scope { cwds }
    }

    pub fn is_global(&self) -> bool {
        self.cwds.is_empty()
    }

    /// What to print in the header, e.g. `~/workspace/chamod/tools/magical`.
    pub fn label(&self, home: Option<&str>) -> String {
        match self.cwds.first() {
            None => String::new(),
            Some(p) => {
                let s = p.to_string_lossy().to_string();
                match home {
                    Some(h) if !h.is_empty() && s.starts_with(h) => format!("~{}", &s[h.len()..]),
                    _ => s,
                }
            }
        }
    }

    /// Is a `cwd` recorded in a transcript at or below our directory?
    pub fn contains(&self, recorded: &str) -> bool {
        if self.is_global() {
            return true;
        }
        let cand = Path::new(recorded);
        self.cwds.iter().any(|base| at_or_below(base, cand))
    }
}

pub fn home_dir() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_at(dir: &str) -> Scope {
        Scope {
            cwds: vec![PathBuf::from(dir)],
        }
    }

    #[test]
    fn exact_and_descendant_cwds_are_in_scope() {
        let s = scope_at("/Users/me/repo/services/api");
        assert!(s.contains("/Users/me/repo/services/api"));
        assert!(s.contains("/Users/me/repo/services/api/tests"));
    }

    #[test]
    fn ancestors_and_siblings_are_out_of_scope() {
        let s = scope_at("/Users/me/repo/services/api");
        assert!(!s.contains("/Users/me/repo"));
        assert!(!s.contains("/Users/me/repo/services/web"));
    }

    #[test]
    fn sibling_prefix_is_not_a_descendant() {
        // The trap a naive string prefix falls into.
        let s = scope_at("/a/b");
        assert!(s.contains("/a/b/c"));
        assert!(!s.contains("/a/bc"));
    }

    #[test]
    fn symlink_aliases_are_both_accepted() {
        let s = Scope {
            cwds: vec![
                PathBuf::from("/private/var/x/repo"),
                PathBuf::from("/var/x/repo"),
            ],
        };
        assert!(s.contains("/private/var/x/repo/sub"));
        assert!(s.contains("/var/x/repo/sub"));
        assert!(!s.contains("/var/x/other"));
    }

    #[test]
    fn label_shortens_home() {
        let s = scope_at("/Users/me/repo");
        assert_eq!(s.label(Some("/Users/me")), "~/repo");
        assert_eq!(s.label(None), "/Users/me/repo");
    }
}
