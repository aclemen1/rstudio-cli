# Project notes

Things an agent can't deduce from the codebase alone. Keep this minimal.

## Release pipeline (CLI repo → Homebrew tap repo)

A bump touches two repos. The CLI binary and the embedded Claude Code
skill share a single `Cargo.toml` `version` (substituted into the
embedded skill markdown via `__VERSION__` at compile time).

1. Bump `Cargo.toml`, add a `[X.Y.Z] — YYYY-MM-DD` section to
   `CHANGELOG.md`, update any `README.md` example that hard-codes a
   version (e.g. `rstudio version # X.Y.Z`).
2. Commit on `main` of `aclemen1/rstudio-cli` (Conventional Commits,
   no `Co-Authored-By` trailers).
3. Push tag `vX.Y.Z`. This triggers `.github/workflows/release.yml`,
   which builds 4 targets
   (`{x86_64,aarch64}-{unknown-linux-gnu,apple-darwin}`) and publishes
   the GitHub Release with `tar.gz` artifacts.
4. Wait for the Actions run to publish the release (≈10 min;
   `gh run watch`, then `gh release view vX.Y.Z`).
5. Update tap `aclemen1/homebrew-tap`, file `Formula/rstudio-cli.rb`:
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
