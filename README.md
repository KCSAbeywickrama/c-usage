# c-usage

How much of your weekly Claude limit the project you're in has used. A command-line tool for [Claude Code](https://claude.com/claude-code).

> Not affiliated with or endorsed by Anthropic. Claude is a trademark of Anthropic.

```
$ cd ~/workspace/chamod/tools/branch-graph-rust
$ cusage
c-usage  ~/workspace/chamod/tools/branch-graph-rust
week  Tue Oct 20 3:29pm → Tue Oct 27 3:29pm

             account   this project   share of local use
all models       47%          ~3.0%                 6.4%
Fable            75%          ~0.0%                 0.0%

this project    40.7M tokens
  haiku-5-5           138.7K
  opus-5-5             40.5M
  sonnet-5-5           38.1K
all projects   326.3M tokens
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/KCSAbeywickrama/c-usage/main/install.sh | sh
```

This installs `c-usage` and the short alias `cusage` into `~/.local/bin`.
- It downloads the prebuilt binary for your platform from GitHub Releases and verifies it against the release's `SHA256SUMS` before installing.
- On a platform with no prebuilt binary, or when the download fails, it builds from source with cargo.

| | |
|---|---|
| `BINDIR=/usr/local/bin ./install.sh` | install somewhere else |
| `VERSION=v0.1.0 ./install.sh` | pin a release |
| `FROM_SOURCE=1 ./install.sh` | skip the download and build locally |
| `./install-dev.sh` | symlink both names to `target/release`, for working on the tool |
| `./uninstall.sh` | remove both names |

Reading the weekly limit needs Claude Code (`claude`) on your PATH.

## How it works

Anthropic only reports the weekly limit for the whole account, so the tool combines two sources:

1. **Account-wide weekly % and reset time.** It runs `claude -p --no-session-persistence "/usage"`. This makes no model call, so no usage is spent, and leaves no transcript behind. It takes about 4 seconds.
2. **Per-project usage for that week.** It reads the transcripts under `~/.claude/projects`, including subagent transcripts.
   - Each API response is counted once. Claude Code logs one record per content block, and resumed or forked sessions copy earlier records.
   - A response belongs to the project when its recorded `cwd` is the current folder or a folder below it.

**share of local use** is this project's part of all local Claude Code usage in the same week, with each token weighted by its API list price. Raw token counts would mislead: most tokens are cheap cache reads, and a Fable token weighs far more than a Haiku one.
- The weights are list prices from the claude-api reference (cached 2026-10-06), in `src/pricing.rs`.
- They are used only for the comparison and never shown as dollars, because on a subscription they aren't what you pay.
- A model with no price is listed and left out of the share.

**this project** (marked `~`) is share × account %. It is an estimate: usage from claude.ai, the desktop or mobile apps, or other machines counts toward the same limit but isn't in the local logs. If you have any, the project's real % is lower.

A separate weekly limit, such as `Current week (Fable)`, gets its own row. That row only counts the models whose id contains that name.

## Fallback

If `claude` can't be run, times out after 30s, or `/usage` prints no weekly line (for example on an API key instead of a subscription), the tool still runs. It shows the share of local use over the last 7 days and the reason it couldn't read `/usage`. `/usage` output is meant for people, so a future Claude Code version could change its format; this is the path you'd see if that happens.

## Options and environment

| | |
|---|---|
| `--no-color` / `--color` | force colour off or on |
| `CLAUDE_PROJECTS_DIR` | transcripts root (default `~/.claude/projects`) |
| `CUSAGE_CLAUDE_BIN` | the `claude` executable to run (default `claude`) |
| `NO_COLOR`, `FORCE_COLOR` | the usual |

## Releasing

1. Bump `version` in `Cargo.toml` and commit, so `Cargo.lock` matches.
2. Tag the commit and push the tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` then does the rest:
- Builds macOS (arm64, x86_64) and static Linux (x86_64, arm64) binaries from the committed `Cargo.lock`.
- Writes `SHA256SUMS`.
- Creates the GitHub Release that `install.sh` downloads from.

To rebuild an existing tag, run the workflow manually with that tag.

## Tests

```sh
cargo test
```

The unit tests cover:
- the `/usage` parser, using real output
- time zones, year rollover and time-only resets
- pricing
- project scoping

`tests/cli.rs` runs the binary against fixture transcripts and a fake `claude`. The fixtures include:
- a response repeated within a file and copied into a forked file
- a subagent file
- a sibling folder
- an out-of-window record
- a `<synthetic>` record
- the fallback path

`src/scope.rs`, `src/time.rs` and `src/term.rs` are adapted from claude-find.

## License

MIT
