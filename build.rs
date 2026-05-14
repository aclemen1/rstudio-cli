//! Build-time orchestration for the embedded `rstudiocli.mcp` R package.
//!
//! Two jobs:
//!
//! 1. Enforce that the Cargo crate version and the R package
//!    `DESCRIPTION::Version` are in lock-step. A drift between the two
//!    would mean a binary that ships an R package claiming to be a
//!    different version — silent footgun. Compilation fails loudly
//!    instead.
//!
//! 2. Produce the source tarball of the R package in `OUT_DIR` so
//!    `include_bytes!` can embed it in the binary. We prefer
//!    `R CMD build` (which strips junk, validates structure, and is
//!    what an R user would do) and fall back to plain `tar` when R
//!    is not installed on the build host — useful for first-time
//!    contributors and for cross-compilation environments that don't
//!    necessarily have R.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let r_pkg_dir = manifest_dir.join("r-package");

    // Rebuild whenever any R-package source changes.
    println!("cargo:rerun-if-changed={}", r_pkg_dir.display());
    println!("cargo:rerun-if-changed=Cargo.toml");

    let cargo_version = env!("CARGO_PKG_VERSION");
    let desc_version = read_description_version(&r_pkg_dir.join("DESCRIPTION"))
        .expect("failed to read r-package/DESCRIPTION Version field");
    assert_eq!(
        cargo_version, desc_version,
        "version mismatch: Cargo.toml = {cargo_version}, r-package/DESCRIPTION = {desc_version}. \
         Bump both, or run `cargo run --bin sync-version` (if available)."
    );

    let tarball = out_dir.join("r-package.tar.gz");

    let used_r = if which_r().is_some() {
        run_r_cmd_build(&r_pkg_dir, &out_dir, &tarball, cargo_version)
    } else {
        false
    };

    if !used_r {
        // Fallback: build a plain tarball by hand. Slightly less polished
        // (doesn't strip backup files etc.) but a valid R source package
        // that install.packages(type = "source") accepts.
        fallback_tar_build(&r_pkg_dir, &tarball);
    }

    assert!(
        tarball.exists(),
        "build.rs failed to produce {}",
        tarball.display()
    );
}

fn read_description_version(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("Version:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn which_r() -> Option<PathBuf> {
    Command::new("R")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from("R"))
}

/// Run `R CMD build r-package` from `out_dir` and move the produced
/// tarball to `target`. Returns true on success.
fn run_r_cmd_build(r_pkg_dir: &Path, out_dir: &Path, target: &Path, version: &str) -> bool {
    let status = Command::new("R")
        .args(["CMD", "build", "--no-manual", "--no-build-vignettes"])
        .arg(r_pkg_dir)
        .current_dir(out_dir)
        .status();
    let ok = matches!(status, Ok(s) if s.success());
    if !ok {
        return false;
    }
    // R CMD build creates rstudiocli.mcp_<version>.tar.gz in CWD.
    let produced = out_dir.join(format!("rstudiocli.mcp_{version}.tar.gz"));
    if !produced.exists() {
        return false;
    }
    // Move to the canonical filename so include_bytes! has a fixed path.
    let _ = fs::rename(&produced, target);
    target.exists()
}

/// Plain-tar fallback when R isn't available on the build host. Builds
/// `rstudiocli.mcp/...` rooted at the package name, which is what
/// `install.packages(..., type = "source")` expects from a tarball.
fn fallback_tar_build(r_pkg_dir: &Path, target: &Path) {
    let parent = r_pkg_dir
        .parent()
        .expect("r-package must have a parent directory");
    // Stage the package under its canonical name ("rstudiocli.mcp"), not
    // "r-package", so tar entries are prefixed correctly.
    let stage = parent.join("target").join("r-package-stage");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).expect("create stage dir");
    let staged_pkg = stage.join("rstudiocli.mcp");
    copy_dir_recursive(r_pkg_dir, &staged_pkg).expect("stage r-package");

    let status = Command::new("tar")
        .args(["-czf"])
        .arg(target)
        .arg("-C")
        .arg(&stage)
        .arg("rstudiocli.mcp")
        .status()
        .expect("invoke tar");
    assert!(status.success(), "tar fallback failed");

    let _ = fs::remove_dir_all(&stage);
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
