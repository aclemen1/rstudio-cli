//! Embedded `rstudiocli.mcp` R package: ship + auto-install.
//!
//! The package source tarball is embedded at compile time via
//! `include_bytes!` (see `build.rs` for how the tarball is produced).
//! On first contact with an rsession we ensure a matching version is
//! installed; if not, we install it silently from the embedded bytes.
//!
//! Policy: install on first use, no confirmation. An rstudio-cli user
//! has already opted in to a binary that drives their R session — a
//! pure-R package installed in their user library is strictly less
//! invasive than the alternatives we considered (injecting an
//! environment via `attach()`, sourcing code into `.GlobalEnv`, etc.).
//!
//! The check is memoised per-process: a single `rstudio` invocation
//! pays at most one round-trip with rsession to verify the version,
//! plus the one-shot install on a clean session.

use std::io::Write;
use std::sync::OnceLock;

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::RpcClient;

/// Embedded source tarball of the `rstudiocli.mcp` R package. Built
/// by `build.rs` (either via `R CMD build` or a tar fallback) and
/// dropped into `OUT_DIR/r-package.tar.gz`.
const R_PACKAGE_TARBALL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/r-package.tar.gz"));

/// The version of the R package that ships with this binary. Verified
/// against `Cargo.toml` at compile-time (build.rs) so we don't need a
/// separate constant: they're guaranteed equal.
const R_PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

const R_PACKAGE_NAME: &str = "rstudiocli.mcp";

/// Memoise "we already ensured installation in this process" so that
/// chains of CLI invocations or MCP tool calls don't repeat the
/// version-check round-trip.
static ENSURED: OnceLock<()> = OnceLock::new();

/// Ensure the rsession has `rstudiocli.mcp` installed at the version
/// shipped with this binary. Silent on success; returns an error only
/// if both the version check and the install fail.
///
/// Safe to call from the hot path: short-circuits after the first
/// successful call within a single process.
pub fn ensure_installed(rpc: &RpcClient<'_>) -> Result<(), CliError> {
    if ENSURED.get().is_some() {
        return Ok(());
    }
    let status = check_installed(rpc)?;
    if matches!(status, InstallStatus::CurrentVersion) {
        let _ = ENSURED.set(());
        return Ok(());
    }
    install_from_embedded(rpc)?;
    let _ = ENSURED.set(());
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum InstallStatus {
    /// Already installed at the expected version; nothing to do.
    CurrentVersion,
    /// Either not installed, or installed at a different version.
    NeedsInstall,
}

fn check_installed(rpc: &RpcClient<'_>) -> Result<InstallStatus, CliError> {
    let probe = format!(
        "if (!requireNamespace({pkg}, quietly = TRUE)) {{\n  \
            cat('missing')\n\
         }} else if (as.character(utils::packageVersion({pkg})) != {ver}) {{\n  \
            cat('mismatch:', as.character(utils::packageVersion({pkg})))\n\
         }} else {{\n  \
            cat('ok')\n\
         }}",
        pkg = r_quote(R_PACKAGE_NAME),
        ver = r_quote(R_PACKAGE_VERSION),
    );
    let raw = r_eval::run(rpc, &probe)?;
    let trimmed = raw.trim();
    // execute_r_code wraps stdout into "[1] \"<value>\"" when the expression
    // is the value itself. We use cat() so the raw output is unwrapped.
    if trimmed == "ok" {
        Ok(InstallStatus::CurrentVersion)
    } else {
        Ok(InstallStatus::NeedsInstall)
    }
}

fn install_from_embedded(rpc: &RpcClient<'_>) -> Result<(), CliError> {
    // Write the embedded tarball to a temp path. We use the rstudio-cli
    // binary's own tmp; rsession reads from the same filesystem, so the
    // path is reachable from the R side.
    let mut tmp = tempfile::Builder::new()
        .prefix("rstudiocli.mcp-")
        .suffix(".tar.gz")
        .tempfile()
        .map_err(|e| CliError::internal(format!("r_package: tempfile: {e}")))?;
    tmp.write_all(R_PACKAGE_TARBALL)
        .map_err(|e| CliError::internal(format!("r_package: write tarball: {e}")))?;
    tmp.flush()
        .map_err(|e| CliError::internal(format!("r_package: flush tarball: {e}")))?;
    let path = tmp.into_temp_path();
    let path_str = path.to_string_lossy().into_owned();

    // install.packages with repos = NULL and type = "source" installs
    // from a local tarball. We deliberately let R pick the user library;
    // overriding lib= here would be surprising for the user.
    let install_code = format!(
        "withCallingHandlers(\n  \
            utils::install.packages({tarball}, repos = NULL, type = 'source', quiet = TRUE),\n  \
            warning = function(w) invokeRestart('muffleWarning'),\n  \
            message = function(m) invokeRestart('muffleMessage')\n\
         )",
        tarball = r_quote(&path_str)
    );
    r_eval::run_silent(rpc, &install_code).map_err(|e| {
        CliError::internal(format!(
            "r_package: failed to install {}: {e}",
            R_PACKAGE_NAME
        ))
    })?;

    // path drops here -> tempfile is removed automatically.
    drop(path);
    Ok(())
}

/// R-style string literal with single-quote escaping. Adequate for our
/// use cases (package names, version strings, filesystem paths).
fn r_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tarball_is_nonempty() {
        // Sanity: build.rs really did put bytes here.
        assert!(!R_PACKAGE_TARBALL.is_empty());
        // gzip magic
        assert_eq!(&R_PACKAGE_TARBALL[..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn r_quote_escapes_quotes_and_backslashes() {
        assert_eq!(r_quote("hello"), "'hello'");
        assert_eq!(r_quote("it's"), r"'it\'s'");
        assert_eq!(r_quote(r"\path"), r"'\\path'");
    }

    #[test]
    fn version_constant_matches_cargo() {
        assert_eq!(R_PACKAGE_VERSION, env!("CARGO_PKG_VERSION"));
    }
}
