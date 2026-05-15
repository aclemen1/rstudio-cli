//! `rstudio project ...` — project lifecycle commands.
//!
//! Five actions:
//!
//! - `current` — return the active project path (was `session project`).
//! - `open <path>` — open an existing project (was `session open-project`).
//! - `new <path>` — create a new project in a NEW directory.
//! - `init <path>` — make an EXISTING directory an RStudio project.
//! - `clone <url> [<path>]` — git clone, then init as a project, then open.
//!
//! `current` and `open` are RPC wrappers around `rstudioapi::*`. The
//! three creation commands (`new` / `init` / `clone`) write the
//! `.Rproj` file from a static template (no R needed for that), then
//! invoke `open` for the switch — so a single `rstudio project new
//! /tmp/x` is "create directory, write .Rproj, switch session".
//!
//! These commands all mutate state and therefore acquire the per-call
//! mutex via the standard dispatch path; wrap them in `rstudio tx --
//! …` (CLI) or `tx_begin` / `tx_end` (MCP) when chaining with other
//! writes that depend on this project being active.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};
use crate::session::{Session, SessionOverrides};

/// Default `.Rproj` body — mirrors what RStudio's File → New Project
/// dialog writes when no advanced options are tweaked. The leading
/// `Version: 1.0` is mandatory for RStudio to recognise the file.
const RPROJ_TEMPLATE: &str = "Version: 1.0\n\
\n\
RestoreWorkspace: Default\n\
SaveWorkspace: Default\n\
AlwaysSaveHistory: Default\n\
\n\
EnableCodeIndexing: Yes\n\
UseSpacesForTab: Yes\n\
NumSpacesForTab: 2\n\
Encoding: UTF-8\n\
\n\
RnwWeave: Sweave\n\
LaTeX: pdfLaTeX\n";

const DEFAULT_GITIGNORE: &str = ".Rproj.user\n.Rhistory\n.RData\n.Ruserdata\n";

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "project",
        name: "current",
        summary: "Return the path of the active RStudio project (null if none).",
        description: "Wraps rstudioapi::getActiveProject(). Was `session project` in v0.9.x.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio project current",
            explanation: "Returns {path: '/path/to/project'} or {path: null}.",
        }],
        returns: "{path: string|null}",
        errors: &[],
        rstudioapi_fn: Some("getActiveProject"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "project",
        name: "open",
        summary: "Open an existing RStudio project (DISRUPTIVE: switches the session context).",
        description: "Wraps rstudioapi::openProject(path, newSession). When --new-session is passed \
             the project opens in a new RStudio session; otherwise it replaces the current \
             one (the R session restarts). Was `session open-project` in v0.9.x.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: ".Rproj path or project root directory.",
            },
            ParamSpec {
                name: "--new-session",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Open in a new RStudio session instead of replacing the current one.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio project open ~/projects/foo",
            explanation: "Replace the current session by foo's project (current R state lost).",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Path not found.",
        }],
        rstudioapi_fn: Some("openProject"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "project",
        name: "new",
        summary: "Create a new RStudio project in a NEW directory and (by default) open it.",
        description: "Creates the directory, writes a default `<basename>.Rproj` template, optionally \
             scaffolds a basic structure (`R/`, `README.md`, `.gitignore`), optionally runs \
             `git init`, and (by default) switches the current session to the new project. \
             Refuses if the path already exists — use `init` for an existing directory. \
             Pure-Rust implementation; doesn't require usethis or any R package.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Directory to create. MUST NOT already exist.",
            },
            ParamSpec {
                name: "--scaffold",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Also create R/, README.md, .gitignore.",
            },
            ParamSpec {
                name: "--git",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Run `git init` in the new directory.",
            },
            ParamSpec {
                name: "--no-open",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Don't switch the current session to the new project.",
            },
            ParamSpec {
                name: "--new-session",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Open in a new RStudio session instead of replacing the current one.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio project new /tmp/my-analysis",
                explanation: "Create + open. Returns {path, rproj, opened: true}.",
            },
            ExampleSpec {
                cmd: "rstudio project new ~/work/foo --scaffold --git",
                explanation: "Create with R/ + README + .gitignore + git repo, then open.",
            },
            ExampleSpec {
                cmd: "rstudio project new /tmp/quick --no-open",
                explanation: "Create only — useful when scripting many projects.",
            },
        ],
        returns: "{path, rproj, scaffolded, git_initialized, opened}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Path already exists.",
            },
            ErrorSpec {
                kind: "user_error",
                when: "`git init` fails (when --git is passed and git is not installed).",
            },
        ],
        rstudioapi_fn: Some("openProject"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "project",
        name: "init",
        summary: "Make an EXISTING directory an RStudio project (writes a .Rproj).",
        description: "Validates that the path is an existing directory, writes a default \
             `<basename>.Rproj` (refuses if one already exists), optionally scaffolds \
             missing R/ + README.md + .gitignore, optionally runs `git init` if no `.git` \
             yet, and (by default) opens the project. Use this to upgrade a plain directory \
             with R code into a proper RStudio project.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Existing directory to make a project of.",
            },
            ParamSpec {
                name: "--scaffold",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Add R/, README.md, .gitignore where missing.",
            },
            ParamSpec {
                name: "--git",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Run `git init` if no .git yet (no-op if already a git repo).",
            },
            ParamSpec {
                name: "--no-open",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Don't switch the current session to the project.",
            },
            ParamSpec {
                name: "--new-session",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Open in a new RStudio session instead of replacing the current one.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio project init ~/legacy-r-code",
                explanation: "Add a .Rproj to an existing R workspace and open it.",
            },
            ExampleSpec {
                cmd: "rstudio project init ~/work/snippets --scaffold --git",
                explanation: "Add structure + git to an existing dir.",
            },
        ],
        returns: "{path, rproj, scaffolded, git_initialized, opened}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Path doesn't exist or is not a directory.",
            },
            ErrorSpec {
                kind: "user_error",
                when: "Directory already contains a *.Rproj file.",
            },
        ],
        rstudioapi_fn: Some("openProject"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "project",
        name: "clone",
        summary: "Clone a Git repository, ensure it's an RStudio project, and open it.",
        description: "Runs `git clone <url> [<path>]` to a destination directory (auto-derived from \
             URL if not given). After cloning: if the working tree has no .Rproj, writes a \
             default one. Then (by default) opens the project. Use this to bring an external \
             R codebase into your RStudio session in one command.",
        params: &[
            ParamSpec {
                name: "url",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Git URL — passed verbatim to `git clone`.",
            },
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Destination directory; defaults to the URL's basename minus `.git`.",
            },
            ParamSpec {
                name: "--no-open",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Don't switch the current session to the cloned project.",
            },
            ParamSpec {
                name: "--new-session",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Open in a new RStudio session instead of replacing the current one.",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio project clone https://github.com/tidyverse/dplyr.git",
                explanation: "Clone into ./dplyr/ and open. Adds a .Rproj if missing.",
            },
            ExampleSpec {
                cmd: "rstudio project clone git@github.com:user/work.git ~/work",
                explanation: "Clone into a specific path.",
            },
            ExampleSpec {
                cmd: "rstudio project clone URL --no-open",
                explanation: "Clone only — useful when scripting batches.",
            },
        ],
        returns: "{path, url, rproj_created, opened}",
        errors: &[
            ErrorSpec {
                kind: "user_error",
                when: "Destination already exists.",
            },
            ErrorSpec {
                kind: "user_error",
                when: "`git clone` fails (network, auth, missing git binary).",
            },
        ],
        rstudioapi_fn: Some("openProject"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum ProjectCmd {
    /// Return the active RStudio project path (or null).
    Current,
    /// Open an existing RStudio project (replaces current session unless --new-session).
    Open {
        path: String,
        /// Open in a new session instead of replacing the current one.
        #[arg(long)]
        new_session: bool,
    },
    /// Create a new RStudio project in a NEW directory.
    New {
        /// Path of the directory to create. MUST NOT already exist.
        path: String,
        /// Also create a basic structure: `R/`, `README.md`, `.gitignore`.
        #[arg(long)]
        scaffold: bool,
        /// Run `git init` in the new project directory.
        #[arg(long)]
        git: bool,
        /// Skip switching the current session to the newly created project.
        #[arg(long)]
        no_open: bool,
        /// When opening, open in a new RStudio session instead of replacing the current one.
        #[arg(long)]
        new_session: bool,
    },
    /// Make an EXISTING directory an RStudio project (writes a .Rproj).
    Init {
        /// Path of an existing directory to make a project of.
        path: String,
        /// Also scaffold `R/`, `README.md`, `.gitignore` where missing.
        #[arg(long)]
        scaffold: bool,
        /// Run `git init` if no `.git` yet.
        #[arg(long)]
        git: bool,
        /// Skip switching the current session to the project.
        #[arg(long)]
        no_open: bool,
        /// When opening, open in a new RStudio session instead of replacing the current one.
        #[arg(long)]
        new_session: bool,
    },
    /// Clone a Git repository, ensure it's an RStudio project, and open it.
    Clone {
        /// Git URL to clone (https://, git@, ssh:// — passed verbatim to `git clone`).
        url: String,
        /// Destination directory; defaults to the basename of the URL with `.git` stripped.
        path: Option<String>,
        /// Skip switching the current session to the cloned project.
        #[arg(long)]
        no_open: bool,
        /// When opening, open in a new RStudio session instead of replacing the current one.
        #[arg(long)]
        new_session: bool,
    },
}

/// Top-level dispatcher. Takes `overrides` instead of a pre-built RPC
/// client so that `new` / `init` / `clone` with `--no-open` (which
/// touch the filesystem only) work even when no RStudio session is
/// running. Session detection happens lazily, inside each variant.
pub fn run(cmd: &ProjectCmd, overrides: SessionOverrides) -> Result<Option<Value>, CliError> {
    match cmd {
        ProjectCmd::Current => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            current(&rpc)
        }
        ProjectCmd::Open { path, new_session } => {
            let session = Session::detect(overrides)?;
            let rpc = RpcClient::new(&session);
            open(&rpc, path, *new_session)
        }
        ProjectCmd::New {
            path,
            scaffold,
            git,
            no_open,
            new_session,
        } => new(overrides, path, *scaffold, *git, !no_open, *new_session),
        ProjectCmd::Init {
            path,
            scaffold,
            git,
            no_open,
            new_session,
        } => init(overrides, path, *scaffold, *git, !no_open, *new_session),
        ProjectCmd::Clone {
            url,
            path,
            no_open,
            new_session,
        } => clone(overrides, url, path.as_deref(), !no_open, *new_session),
    }
}

/// Detect session + build RPC, used from `new` / `init` / `clone`
/// only when `open_after` is true (saves the cost when scripts call
/// these with `--no-open`).
fn detect_rpc(overrides: SessionOverrides) -> Result<Session, CliError> {
    Session::detect(overrides)
}

fn current(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/project.R`.
    let r_code = r#"local({
  p <- rstudiocli::project_current()
  if (is.null(p)) cat("{\"path\":null}")
  else cat(jsonlite::toJSON(list(path = p), auto_unbox = TRUE))
})"#;
    let raw = r_eval::run(rpc, r_code)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("project current: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn open(rpc: &RpcClient<'_>, path: &str, new_session: bool) -> Result<Option<Value>, CliError> {
    let new_arg = if new_session { "TRUE" } else { "FALSE" };
    let r_code = format!(
        "rstudiocli::project_open(path = {}, new_session = {new_arg})",
        r_quote(path)
    );
    r_eval::run_silent(rpc, &r_code)?;
    Ok(None)
}

fn new(
    overrides: SessionOverrides,
    path: &str,
    scaffold: bool,
    git: bool,
    open_after: bool,
    new_session: bool,
) -> Result<Option<Value>, CliError> {
    let dir = PathBuf::from(path);
    if dir.exists() {
        return Err(CliError::user(format!(
            "{} already exists; use `project init` for an existing directory",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| CliError::user(format!("cannot create {}: {e}", dir.display())))?;
    let rproj = write_rproj_file(&dir)?;
    if scaffold {
        scaffold_dir(&dir)?;
    }
    if git {
        git_init(&dir)?;
    }
    let canon = canonicalize(&dir)?;
    let opened = if open_after {
        let session = detect_rpc(overrides)?;
        let rpc = RpcClient::new(&session);
        open(&rpc, &canon.to_string_lossy(), new_session)?;
        true
    } else {
        false
    };
    Ok(Some(json!({
        "path": canon.display().to_string(),
        "rproj": rproj.display().to_string(),
        "scaffolded": scaffold,
        "git_initialized": git,
        "opened": opened,
    })))
}

fn init(
    overrides: SessionOverrides,
    path: &str,
    scaffold: bool,
    git: bool,
    open_after: bool,
    new_session: bool,
) -> Result<Option<Value>, CliError> {
    let dir = PathBuf::from(path);
    if !dir.exists() {
        return Err(CliError::user(format!(
            "{} does not exist; use `project new` to create it",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(CliError::user(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if has_rproj_file(&dir) {
        return Err(CliError::user(format!(
            "{} already contains a .Rproj file; use `project open` to open it",
            dir.display()
        )));
    }
    let rproj = write_rproj_file(&dir)?;
    if scaffold {
        scaffold_dir(&dir)?;
    }
    let did_git_init = if git && !dir.join(".git").exists() {
        git_init(&dir)?;
        true
    } else {
        false
    };
    let canon = canonicalize(&dir)?;
    let opened = if open_after {
        let session = detect_rpc(overrides)?;
        let rpc = RpcClient::new(&session);
        open(&rpc, &canon.to_string_lossy(), new_session)?;
        true
    } else {
        false
    };
    Ok(Some(json!({
        "path": canon.display().to_string(),
        "rproj": rproj.display().to_string(),
        "scaffolded": scaffold,
        "git_initialized": did_git_init,
        "opened": opened,
    })))
}

fn clone(
    overrides: SessionOverrides,
    url: &str,
    path: Option<&str>,
    open_after: bool,
    new_session: bool,
) -> Result<Option<Value>, CliError> {
    let dest = match path {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(derive_clone_path(url)),
    };
    if dest.exists() {
        return Err(CliError::user(format!("{} already exists", dest.display())));
    }
    git_clone(url, &dest)?;
    let rproj_created = if has_rproj_file(&dest) {
        None
    } else {
        Some(write_rproj_file(&dest)?.display().to_string())
    };
    let canon = canonicalize(&dest)?;
    let opened = if open_after {
        let session = detect_rpc(overrides)?;
        let rpc = RpcClient::new(&session);
        open(&rpc, &canon.to_string_lossy(), new_session)?;
        true
    } else {
        false
    };
    Ok(Some(json!({
        "path": canon.display().to_string(),
        "url": url,
        "rproj_created": rproj_created,
        "opened": opened,
    })))
}

// -----------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------

fn write_rproj_file(dir: &Path) -> Result<PathBuf, CliError> {
    let name = dir.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        CliError::user(format!(
            "cannot derive project name from path {}",
            dir.display()
        ))
    })?;
    let rproj = dir.join(format!("{name}.Rproj"));
    if rproj.exists() {
        return Err(CliError::user(format!(
            "project file already exists: {}",
            rproj.display()
        )));
    }
    std::fs::write(&rproj, RPROJ_TEMPLATE)
        .map_err(|e| CliError::user(format!("cannot write {}: {e}", rproj.display())))?;
    Ok(rproj)
}

fn scaffold_dir(dir: &Path) -> Result<(), CliError> {
    let r_dir = dir.join("R");
    if !r_dir.exists() {
        std::fs::create_dir_all(&r_dir)
            .map_err(|e| CliError::user(format!("cannot create R/: {e}")))?;
    }
    let readme = dir.join("README.md");
    if !readme.exists() {
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project");
        std::fs::write(&readme, format!("# {name}\n"))
            .map_err(|e| CliError::user(format!("cannot write README.md: {e}")))?;
    }
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, DEFAULT_GITIGNORE)
            .map_err(|e| CliError::user(format!("cannot write .gitignore: {e}")))?;
    }
    Ok(())
}

fn git_init(dir: &Path) -> Result<(), CliError> {
    let status = StdCommand::new("git")
        .args(["init", "-q"])
        .arg(dir)
        .status()
        .map_err(|e| CliError::user(format!("`git init` failed to spawn: {e}")))?;
    if !status.success() {
        return Err(CliError::user(format!(
            "`git init` exited with code {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

fn git_clone(url: &str, dest: &Path) -> Result<(), CliError> {
    let status = StdCommand::new("git")
        .arg("clone")
        .arg(url)
        .arg(dest)
        .status()
        .map_err(|e| CliError::user(format!("`git clone` failed to spawn: {e}")))?;
    if !status.success() {
        return Err(CliError::user(format!(
            "`git clone` exited with code {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

fn canonicalize(p: &Path) -> Result<PathBuf, CliError> {
    p.canonicalize()
        .map_err(|e| CliError::user(format!("canonicalize {}: {e}", p.display())))
}

fn has_rproj_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .to_lowercase()
            .ends_with(".rproj")
    })
}

/// `https://github.com/foo/bar.git` → `bar`
/// `git@github.com:foo/bar.git` → `bar`
/// `bar` → `bar`
fn derive_clone_path(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or("project");
    last.trim_end_matches(".git").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_clone_path_handles_common_url_shapes() {
        assert_eq!(derive_clone_path("https://github.com/foo/bar.git"), "bar");
        assert_eq!(derive_clone_path("git@github.com:foo/bar.git"), "bar");
        assert_eq!(derive_clone_path("ssh://git@host/foo/bar"), "bar");
        assert_eq!(derive_clone_path("https://example.com/proj/"), "proj");
        assert_eq!(derive_clone_path("just-a-name"), "just-a-name");
    }

    #[test]
    fn write_rproj_file_creates_template() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj-name");
        std::fs::create_dir_all(&dir).unwrap();
        let rproj = write_rproj_file(&dir).unwrap();
        assert_eq!(rproj.file_name().unwrap(), "proj-name.Rproj");
        let content = std::fs::read_to_string(&rproj).unwrap();
        assert!(content.starts_with("Version: 1.0"));
        assert!(content.contains("UseSpacesForTab: Yes"));
    }

    #[test]
    fn write_rproj_file_refuses_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        write_rproj_file(&dir).unwrap();
        let err = write_rproj_file(&dir).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn scaffold_dir_creates_expected_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scaff");
        std::fs::create_dir_all(&dir).unwrap();
        scaffold_dir(&dir).unwrap();
        assert!(dir.join("R").is_dir());
        assert!(dir.join("README.md").is_file());
        assert!(dir.join(".gitignore").is_file());
        // Idempotent.
        scaffold_dir(&dir).unwrap();
    }

    #[test]
    fn has_rproj_detects_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(!has_rproj_file(dir));
        std::fs::write(dir.join("Foo.Rproj"), "Version: 1.0\n").unwrap();
        assert!(has_rproj_file(dir));
    }
}
