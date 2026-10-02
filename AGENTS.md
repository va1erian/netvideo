# Agent & contributor guidelines

netvideo is a home-lab video streaming server in Rust (Linux, Docker, behind Cosmos Cloud) with an Android client and an XUI desktop client for Windows and macOS. The plan is [docs/PLAN.md](docs/PLAN.md).

**Keep things small, correct and fast.** Prefer the simplest design that works, prove it with tests, and don't add work the task didn't ask for.

## Code quality
- Keep code clean, structured and readable. Prefer small, focused modules: **aim for files under ~300 lines, hard limit 500** (tests included). Split by responsibility, not arbitrarily.
- Clear names, doc comments on public items, no dead code, no commented-out code, no `println!` debugging (use `tracing`; the CLI's user-facing output is the exception).
- Errors: `thiserror` in library code, `anyhow` only in binaries. No `unwrap()`/`expect()` outside tests except for true invariants (with a message explaining why).
- Match the existing style of the surrounding code.

## Security
- Security is a first-class requirement. Every file the server opens goes through the path jail (`security::path_jail`); clients never send paths.
- Never put secrets in URLs, logs or error messages. Internal errors return a generic message; security events go to the audit log (`audit.rs`).
- External processes (ffmpeg, ffprobe) are spawned with an argument vector, never a shell, with inputs passed as `file:` paths that were resolved through the jail.
- New endpoints are authenticated by default (`AuthDevice`); device management needs `AdminDevice`.

## Unsafe
- **Avoid `unsafe`.** Every crate has `#![forbid(unsafe_code)]` in its `lib.rs`/`main.rs`.
- The only planned exception is the libmpv binding for the desktop video widget: there, `unsafe` lives in one small `ffi` module behind a safe API, and every `unsafe` block gets a `// SAFETY:` comment.

## Workspace rules
- Edition 2024, `members = ["crates/*"]`. Declare dependencies in **your crate's own** `Cargo.toml`.
- Code copied from emusic keeps emusic's structure where it fits; rename domain strings (`netvideo/...`) and env prefixes (`NETVIDEO_*`).
- Stay within the task's scope; list follow-ups in the PR description.

## Git workflow (no merge commits)
- Branch from the latest `origin/main`.
- **Always rebase, never merge:** before submitting, `git fetch origin && git rebase origin/main`. On a `Cargo.lock` conflict, take `origin/main`'s version and re-run `cargo check`.
- After rebasing, re-run all checks below, then push with `--force-with-lease`.
- PRs are integrated into `main` with **rebase merge** or **squash merge**, never a merge commit.

## Before submitting (mandatory)
All of these must pass locally — the same checks CI runs:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Do not submit work with failing or skipped checks.
