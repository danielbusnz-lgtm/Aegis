# Development Rules

Rules for humans and agents working in this repo. If you use an agent, run it from the repo root so it picks this file up automatically.

## Conversational Style

- Keep answers short and concise.
- No emojis in commits, issues, PR comments, or code.
- No fluff or cheerful filler text.
- Technical prose only. Be direct.
- When the user asks a question, answer it first before making edits or running implementation commands.
- When responding to user feedback or an analysis, explicitly say whether you agree or disagree before saying what you changed.
- Never use em dashes or en dashes in prose, comments, or commit messages. Use periods, commas, or restructure.

## Code Quality

- Read files in full before wide-ranging changes, before editing files you have not fully inspected, and when asked to investigate or audit. Do not rely on search snippets for broad changes.
- No `unwrap()` or `expect()` in production paths without a `// SAFETY:` or `// reason:` comment explaining why the invariant holds. Tests, demos, evals, and `main()` startup may unwrap freely.
- Prefer `Result<T, E>` over `panic!` outside `main()`.
- Inline single-line helpers that have only one call site.
- Reach for `?` over manual match arms when the only thing happening on the Err side is propagation.
- Don't add `pub` you don't need. Field and function visibility is the smallest interface that compiles.
- Match the existing comment style. Comments describe what code can't say (Ousterhout discipline). Don't restate what's obvious from the symbol names.
- Always ask before removing functionality or code that appears intentional.
- Do not preserve backward compatibility unless the user asks for it.
- Never hardcode tuning constants in the middle of logic. Put them in `aegis/src/tuning.rs` with an `↑` / `↓` tradeoff comment.
- When touching `providers/claude/*`, remember the hot path is voice-latency-bound. Don't add work that runs synchronously before TTS without checking the budget in `tuning.rs`.

## Commands

- After code changes (not docs): `cargo check` (full output, no tail). Fix all errors and warnings before committing.
- After touching anything in `src/`, run `cargo test --bin aegis` and confirm all unit tests pass.
- Don't run `cargo build --release` unless asked. It's slow and unnecessary for verification.
- Don't run evals (`cargo run --bin eval_*`) unless asked. They cost real API credits.
- Don't run demos (`cargo run --bin demo_*` / `bench_*`) unless asked. Many require hardware (mic, screen, hotkey daemon) or live API access.
- For features other than the default (`hyprland`), verify with `cargo check --no-default-features --features <combo>` (e.g. `winit-window,crossplatform`).
- If you create or modify a unit test, run it and iterate on test or implementation until it passes.
- For ad-hoc scripts, write them to `/tmp` (not `~/Scripts/`), run, remove when done. Don't embed multi-line scripts in `bash` commands.
- Never commit unless the user asks.

## Where Things Go

- **Unit tests**: inline in source files via `#[cfg(test)] mod tests`. Run with `cargo test`. Must be deterministic and free.
- **Integration tests**: `aegis/tests/`. Run with `cargo test`. None exist yet but the directory is reserved.
- **Demos**: `aegis/demos/demo_*.rs`. Hand-run dev tools. Each is a `[[bin]]` entry in `Cargo.toml`.
- **Benchmarks**: `aegis/demos/bench_*.rs`. Same shape as demos but report latency stats over N iterations.
- **Evals**: `aegis/evals/runners/*.rs` (runner code) + `aegis/evals/cases/*.json` (case data) + `aegis/evals/results/` (gitignored output). Run with `cargo run --bin eval_<name>`. LLM behavior tests. Stochastic, paid, not part of CI.
- **Providers**: `aegis/src/providers/`. Each external service (Claude, Deepgram, Cartesia, integrations) is its own module.
- **Tuning constants**: `aegis/src/tuning.rs`. Every behavior dial in one place.
- **Memory architecture**: `aegis/docs/memory-architecture.md`. Three-tier design (facts JSONL, history SQLite+FTS5, future embeddings).

## Dependencies

- Treat `Cargo.lock` changes as reviewed code. Direct external deps stay pinned to the minor version (`"1.2.3"`-style entries in `Cargo.toml`).
- Update locally with `cargo update -p <crate>` for targeted bumps. Avoid blanket `cargo update`.
- New dependencies require justification. Prefer pulling in fewer features (`default-features = false`) over the whole crate when possible.

## Git

Multiple agent sessions may be running in this cwd at the same time, each modifying different files. Git operations that touch unstaged, staged, or untracked files outside your own changes will stomp on other sessions' work. Follow these rules:

Committing:

- Only commit files YOU changed in THIS session.
- Stage explicit paths (`git add <path1> <path2>`); never `git add -A` / `git add .`.
- Before committing, run `git status` and verify you are only staging your files.

Never run (destroys other agents' work or bypasses checks):

- `git reset --hard`, `git checkout .`, `git clean -fd`, `git stash`, `git add -A`, `git add .`, `git commit --no-verify`.

Commit message style:

- Lowercase prefix matching the area of change, then a colon, then a terse description. Examples: `history: add sqlite + fts5 conversation log`, `tuning: zero post-release grace`, `demos: move examples/ → demos/`.
- No emoji, no AI-tell words ("comprehensive", "enhance", "streamline", "robust", "leverage").
- No `Co-Authored-By:` lines. Maintainer commits look solo.
- No `Generated with Claude Code` footers.

If rebase conflicts occur:

- Resolve conflicts only in files you modified.
- If a conflict is in a file you did not modify, abort and ask the user.
- Never force push.

## Issues and PRs

See `CONTRIBUTING.md` for the contributor gate (auto-close, `lgtm`/`lgtmi`, quality bar).

When creating issues, add area labels for affected modules:

- `area:voice` (STT, TTS, audio pipeline)
- `area:classifier` (intent routing)
- `area:memory` (facts, history, eval, retrieval)
- `area:agent` (multi-step agent loop)
- `area:integrations` (Gmail, GitHub, Spotify, YouTube)
- `area:ui` (cursor overlay, soundwave, loading)
- `area:platform` (Hyprland, winit, cross-platform)
- `area:build` (Cargo, CI, dependency management)

Use all that apply.

When posting issue/PR comments:

- Write the comment to a temp file and post with `gh issue/pr comment --body-file` (never multi-line markdown via `--body`).
- Keep comments concise, technical, in the user's tone.

When closing issues via commit:

- Include `fixes #<number>` or `closes #<number>` in the message so merging auto-closes the issue. For multiple issues, repeat the keyword per issue (`closes #1, closes #2`); a shared keyword only closes the first.

## Releases

aegis is a binary crate, not a published library. Releases are:

1. Update `aegis/CHANGELOG.md` under the `## [Unreleased]` section (create if missing).
2. Bump `version` in `aegis/Cargo.toml`.
3. Build a release binary: `cargo build --release --bin aegis`.
4. Smoke test the binary on the target platform.
5. Tag the commit (`git tag v0.X.Y`) and push the tag.
6. (Optional) Create a GitHub release with the binary attached.

No npm, no crates.io publish, no 2FA flow.

## User Override

If the user's instructions conflict with any rule in this document, ask for explicit confirmation before overriding. Only then execute their instructions.
