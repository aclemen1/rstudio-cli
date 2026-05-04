# Project notes

Things an agent can't deduce from the codebase alone. Keep this minimal.

## Release pipeline (CLI repo → Homebrew tap repo)

A bump touches two repos. The CLI binary and the embedded Claude Code
skill share a single `Cargo.toml` `version` (substituted into the
embedded skill markdown via `__VERSION__` at compile time).

1. Bump `Cargo.toml`, add a `[X.Y.Z] — YYYY-MM-DD` section to
   `CHANGELOG.md`, update any `README.md` example that hard-codes a
   version (e.g. `rstudio version # X.Y.Z`).
2. **Run the local preflight gauntlet** — same checks as CI on `main`,
   plus `cargo build --release` to mirror what `release.yml` does:

   ```sh
   cargo fmt --check && \
     cargo clippy --all-targets -- -D warnings && \
     cargo test --lib && \
     cargo build --release
   ```

   Why this step exists: `.github/workflows/release.yml` runs only
   `cargo build` (no `cargo clippy`, no `cargo test`). A tag whose
   source has a clippy lint will still produce valid release binaries
   — but the CI workflow on `main` (`.github/workflows/ci.yml`) WILL
   fail on the same commit. Ship 0.8.2 hit exactly this: lint slipped
   through, binaries shipped fine, but CI on `main` went red until a
   follow-up `fix(ci)` commit. Run the gauntlet first; tag second.

3. Commit on `main` of `aclemen1/rstudio-cli` (Conventional Commits,
   no `Co-Authored-By` trailers).
4. Push tag `vX.Y.Z`. This triggers `.github/workflows/release.yml`,
   which builds 4 targets
   (`{x86_64,aarch64}-{unknown-linux-gnu,apple-darwin}`) and publishes
   the GitHub Release with `tar.gz` artifacts.
5. Wait for the Actions run to publish the release (≈10 min;
   `gh run watch`, then `gh release view vX.Y.Z`).
6. Update tap `aclemen1/homebrew-tap`, file `Formula/rstudio-cli.rb`:
   - `gh release download vX.Y.Z -R aclemen1/rstudio-cli -p '*.tar.gz' --dir <tmp>`.
     Don't `curl` the asset URL — GitHub returns a redirect to a signed
     S3 URL needing `gh` auth; raw curl yields a 9-byte error file.
   - `sha256sum <tmp>/*.tar.gz` → 4 hashes.
   - Bump `version` and replace the four `sha256` lines (one per
     `on_arm`/`on_intel` × `on_macos`/`on_linux`).
   - Commit + push the tap's `main`.

## VCS

Both repos are Jujutsu colocated with Git (`.jj/` + `.git/`). Only use
`jj` commands; never raw `git` (corrupts the `.jj/` view).

## Language

Code, comments, README, CHANGELOG, commit messages: English.
Conversation with the user: French.

## Source of truth for non-deducible knowledge

Three documents must contain everything an agent (or a future
contributor) **cannot deduce** from the codebase, the CLI `--help`,
or the schema:

1. **`src/skills/rstudio.md`** — the embedded **CLI skill** that ships
   to Claude Code. For agents that drive the CLI from a shell
   (`rstudio editor read X | jq …`). Installed via `rstudio skill
   install` into the user's global Claude Code `~/.claude/skills/`.
   Talks in terms of `rstudio` invocations, `--format json`, `rstudio
   tx -- bash`, pipes, `--no-lock`, etc.

2. **`src/skills/rstudio-mcp.md`** — the embedded **MCP skill**.
   Returned in the `instructions` field of the MCP server's
   `initialize` response. For agents connected via MCP (Claude Code,
   Cline, Cursor, Continue, Claude Desktop). Talks in terms of MCP
   tool names (`editor_read_buffer`, not `rstudio editor read-buffer`),
   `tx_begin`/`tx_end`/`tx_run` (not `rstudio tx --`), `meta_status`
   for lock visibility (not shell pipes). The two skills overlap on
   semantics — they describe the same RStudio bridge — but the
   vocabulary differs because the surface differs.

3. **`CLAUDE.md`** (this file) — non-deducible facts about the project
   itself: release pipeline, VCS choice, language conventions, code-
   ownership patterns. Things a contributor reading the source tree
   alone would miss or get wrong.

All three should be **minimal**: anything provable from the code or
schema belongs in code/schema, not in these docs. When you add a
feature whose use requires non-obvious knowledge, update the relevant
skill (often both) in the same commit. Keep the two skills in sync on
shared semantics — multi-agent rules in particular must say the same
thing in both places (only the spelling of how to invoke a tx differs).

**Skill change ⇒ version bump.** Both skill files (`src/skills/rstudio.md`
and `src/skills/rstudio-mcp.md`) are baked into the binary at compile
time via `include_str!` and substituted with the binary version at
runtime. Any non-trivial change to either content warrants a new
binary version (patch bump is fine for clarifications; minor for
substantive additions) — otherwise users who installed the skill at
the prior version keep seeing the old text. Trivial typo fixes that
don't change the conveyed information can ship in any next release.

## When adding or changing a CLI command

A user-visible command change is not done until three surfaces agree.
Update them in the same commit (or the same PR for a multi-commit
feature):

1. **`README.md`** — both the command-summary table and the
   *comparative table* against other R/LLM tools (rows reflect what
   the CLI can now do; tick / cross / partial cells change accordingly).
2. **CLI help** — the `///` doc comments on every clap `Command`
   variant, `Subcommand` enum, and `#[arg(...)]` field. Clap uses these
   verbatim for `--help`. Mention defaults and constraints when
   non-obvious; keep summaries one line.
3. **`schema` registry** — the new module's `pub const ACTIONS:
   &[ActionSpec]` must be added to `src/schema.rs` `registry()`, and
   the new category to `CATEGORIES`, so `rstudio schema <cat>` and
   `rstudio schema <cat> <action>` work.

If the change is a removal or a renaming, also update the *Design
philosophy* section of the README when the rationale is non-obvious
(e.g. why `editor find` was dropped in favour of `editor set-marks`).
