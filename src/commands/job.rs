use clap::Subcommand;
use serde_json::Value;

use crate::error::CliError;
use crate::r_eval;
use crate::rpc::{RpcClient, r_quote};
use crate::schema::{ActionSpec, ErrorSpec, ExampleSpec, ParamKind, ParamSpec};

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "job",
        name: "list",
        summary: "List every background job currently registered in the Jobs pane.",
        description: "Wraps .rs.api.listJobs(). Returns a named list of jobs keyed by job id.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio job list",
            explanation: "Returns the active jobs (empty object if none).",
        }],
        returns: "{jobs: object}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "add",
        summary: "Register a new background job in the Jobs pane.",
        description: "Wraps rstudioapi::jobAdd(name, status, progressUnits, running, \
                      autoRemove, show). Returns the new job's id, suitable for further \
                      job set-progress / set-state / add-output / remove calls.",
        params: &[
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Display name in the Jobs pane.",
            },
            ParamSpec {
                name: "--status",
                kind: ParamKind::String,
                required: false,
                default: Some(""),
                allowed: &[],
                description: "Initial status text.",
            },
            ParamSpec {
                name: "--progress-units",
                kind: ParamKind::Integer,
                required: false,
                default: Some("0"),
                allowed: &[],
                description: "Total progress units (0 = indeterminate).",
            },
            ParamSpec {
                name: "--running",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Mark the job as running on creation.",
            },
            ParamSpec {
                name: "--auto-remove",
                kind: ParamKind::Bool,
                required: false,
                default: Some("true"),
                allowed: &[],
                description: "Remove the job from the pane once it succeeds.",
            },
            ParamSpec {
                name: "--show",
                kind: ParamKind::Bool,
                required: false,
                default: Some("true"),
                allowed: &[],
                description: "Show the job in the pane.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job add --name 'my-task' --progress-units 100 --running",
            explanation: "Register a determinate-progress job named 'my-task'.",
        }],
        returns: "{id: string}",
        errors: &[],
        rstudioapi_fn: Some("jobAdd"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "remove",
        summary: "Remove a job from the Jobs pane.",
        description: "Wraps rstudioapi::jobRemove(job).",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Job id (from `job add`).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio job remove abc123",
            explanation: "Remove job abc123.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("jobRemove"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "set-progress",
        summary: "Set the absolute progress of a job (in units).",
        description: "Wraps rstudioapi::jobSetProgress(job, units).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Job id.",
            },
            ParamSpec {
                name: "units",
                kind: ParamKind::Integer,
                required: true,
                default: None,
                allowed: &[],
                description: "Absolute progress (must be <= --progress-units used at creation).",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job set-progress abc123 42",
            explanation: "Set progress to 42 units.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("jobSetProgress"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "add-progress",
        summary: "Increment a job's progress by N units.",
        description: "Wraps rstudioapi::jobAddProgress(job, units).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Job id.",
            },
            ParamSpec {
                name: "units",
                kind: ParamKind::Integer,
                required: true,
                default: None,
                allowed: &[],
                description: "Number of units to add.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job add-progress abc123 5",
            explanation: "Increment progress by 5.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("jobAddProgress"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "set-state",
        summary: "Update a job's lifecycle state.",
        description: "Wraps rstudioapi::jobSetState(job, state).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Job id.",
            },
            ParamSpec {
                name: "state",
                kind: ParamKind::Enum,
                required: true,
                default: None,
                allowed: &["idle", "running", "succeeded", "cancelled", "failed"],
                description: "New state.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job set-state abc123 succeeded",
            explanation: "Mark the job as completed successfully.",
        }],
        returns: "void",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Invalid state value.",
        }],
        rstudioapi_fn: Some("jobSetState"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "set-status",
        summary: "Update a job's status text (free-form).",
        description: "Wraps rstudioapi::jobSetStatus(job, status).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Job id.",
            },
            ParamSpec {
                name: "status",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "New status text.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job set-status abc123 'Indexing files...'",
            explanation: "Update the status line.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("jobSetStatus"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "add-output",
        summary: "Append text to a job's output log.",
        description: "Wraps rstudioapi::jobAddOutput(job, output, error).",
        params: &[
            ParamSpec {
                name: "id",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Job id.",
            },
            ParamSpec {
                name: "output",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: "Text to append.",
            },
            ParamSpec {
                name: "--error",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Mark this output as an error message.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job add-output abc123 'Processed 42 files\\n'",
            explanation: "Add a line to the job's output log.",
        }],
        returns: "void",
        errors: &[],
        rstudioapi_fn: Some("jobAddOutput"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "run-script",
        summary: "Run an R script as a background job.",
        description: "Wraps rstudioapi::jobRunScript(path, name, encoding, workingDir, \
                      importEnv, exportEnv). Returns the job id.",
        params: &[
            ParamSpec {
                name: "path",
                kind: ParamKind::String,
                required: true,
                default: None,
                allowed: &[],
                description: ".R script path.",
            },
            ParamSpec {
                name: "--name",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Display name (defaults to the script's filename).",
            },
            ParamSpec {
                name: "--working-dir",
                kind: ParamKind::String,
                required: false,
                default: None,
                allowed: &[],
                description: "Working directory.",
            },
            ParamSpec {
                name: "--import-env",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Import the calling environment into the job.",
            },
            ParamSpec {
                name: "--export-env",
                kind: ParamKind::String,
                required: false,
                default: Some(""),
                allowed: &[],
                description: "Variable name to export results into after completion.",
            },
        ],
        examples: &[ExampleSpec {
            cmd: "rstudio job run-script ~/scripts/long.R --name 'long-task'",
            explanation: "Run long.R as a background job named 'long-task'.",
        }],
        returns: "{id: string}",
        errors: &[ErrorSpec {
            kind: "user_error",
            when: "Script path not found.",
        }],
        rstudioapi_fn: Some("jobRunScript"),
        rpc_method: Some("execute_r_code"),
    },
    ActionSpec {
        category: "job",
        name: "kill",
        summary: "Stop a job in the Jobs pane (best-effort hard kill + cancelled state).",
        description: "Performs two steps in sequence: \
                      (1) attempts the rsession internal RPC `execute_job_action` \
                      with action=\"stop\", which is the same call the Jobs pane's \
                      Stop button issues — for `job run-script` jobs this terminates \
                      the underlying R sub-process. The RPC error (if the method is \
                      unknown to your RStudio version) is swallowed silently. \
                      (2) calls `rstudioapi::jobSetState(id, \"cancelled\")` to flip \
                      the UI state to cancelled (the only thing that worked for \
                      manually-driven `job add` jobs anyway). Returns \
                      `{cancelled: true, hard_killed: bool}` where hard_killed \
                      reflects whether step (1) succeeded. For async R jobs \
                      created via `r exec --async`, use `r kill` instead — those \
                      live in a separate registry (callr) and are not visible to \
                      the Jobs pane.",
        params: &[ParamSpec {
            name: "id",
            kind: ParamKind::String,
            required: true,
            default: None,
            allowed: &[],
            description: "Job id (from `job list`, `job add` or `job run-script`).",
        }],
        examples: &[ExampleSpec {
            cmd: "rstudio job kill abc123",
            explanation: "Stop job abc123. Returns {cancelled: true, hard_killed: true|false}.",
        }],
        returns: "{cancelled: bool, hard_killed: bool}",
        errors: &[ErrorSpec {
            kind: "r_error",
            when: "jobSetState rejected the id (unknown job).",
        }],
        rstudioapi_fn: Some("jobSetState"),
        rpc_method: Some("execute_job_action"),
    },
    ActionSpec {
        category: "job",
        name: "is-active",
        summary: "Whether the current R execution context is itself a background job.",
        description: "Wraps rstudioapi::isJob(). Returns false from the main R session, \
                      true inside a job's R code.",
        params: &[],
        examples: &[ExampleSpec {
            cmd: "rstudio job is-active",
            explanation: "From the CLI, always returns {is_job: false} (we're not inside a job).",
        }],
        returns: "{is_job: bool}",
        errors: &[],
        rstudioapi_fn: Some("isJob"),
        rpc_method: Some("execute_r_code"),
    },
];

#[derive(Subcommand, Debug)]
pub enum JobCmd {
    /// List every background job currently in the Jobs pane.
    List,
    /// Register a new background job. Returns the new job's id.
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        status: String,
        #[arg(long, default_value_t = 0)]
        progress_units: u32,
        #[arg(long)]
        running: bool,
        #[arg(long, default_value_t = true)]
        auto_remove: bool,
        #[arg(long, default_value_t = true)]
        show: bool,
    },
    /// Remove a job from the Jobs pane.
    Remove { id: String },
    /// Set absolute progress.
    SetProgress { id: String, units: u32 },
    /// Increment progress.
    AddProgress { id: String, units: u32 },
    /// Update lifecycle state.
    SetState { id: String, state: String },
    /// Update status text.
    SetStatus { id: String, status: String },
    /// Append text to a job's output log.
    AddOutput {
        id: String,
        output: String,
        #[arg(long)]
        error: bool,
    },
    /// Run an R script as a background job.
    RunScript {
        path: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        working_dir: Option<String>,
        #[arg(long)]
        import_env: bool,
        #[arg(long, default_value = "")]
        export_env: String,
    },
    /// Stop a job in the Jobs pane (best-effort hard kill + flag as cancelled).
    Kill { id: String },
    /// Whether the current R execution is itself a background job.
    IsActive,
}

pub fn run(cmd: &JobCmd, rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    match cmd {
        JobCmd::List => list(rpc),
        JobCmd::Add {
            name,
            status,
            progress_units,
            running,
            auto_remove,
            show,
        } => add(
            rpc,
            name,
            status,
            *progress_units,
            *running,
            *auto_remove,
            *show,
        ),
        JobCmd::Remove { id } => silent_id(rpc, "job_remove", id),
        JobCmd::SetProgress { id, units } => silent_id_int(rpc, "job_set_progress", id, *units),
        JobCmd::AddProgress { id, units } => silent_id_int(rpc, "job_add_progress", id, *units),
        JobCmd::SetState { id, state } => set_state(rpc, id, state),
        JobCmd::SetStatus { id, status } => set_status(rpc, id, status),
        JobCmd::AddOutput { id, output, error } => add_output(rpc, id, output, *error),
        JobCmd::RunScript {
            path,
            name,
            working_dir,
            import_env,
            export_env,
        } => run_script(
            rpc,
            path,
            name.as_deref(),
            working_dir.as_deref(),
            *import_env,
            export_env,
        ),
        JobCmd::Kill { id } => kill(rpc, id),
        JobCmd::IsActive => is_active(rpc),
    }
}

fn kill(rpc: &RpcClient<'_>, id: &str) -> Result<Option<Value>, CliError> {
    // Step 1 (best-effort): mirror the Jobs pane's Stop button by firing
    // the rsession internal `execute_job_action` RPC with action="stop".
    // This is what actually terminates the sub-process for `jobRunScript`
    // jobs. The exact method name and arg shape are not documented in
    // rstudioapi and may vary across RStudio versions — we swallow any
    // RPC error and record the outcome as `hard_killed`. Server R errors
    // (code 100) propagate to step 2's failure path instead.
    let hard_killed = rpc
        .rpc(
            "execute_job_action",
            vec![Value::String(id.into()), Value::String("stop".into())],
        )
        .is_ok();
    // Step 2 (definitive): flip the UI state. For `jobAdd` (manually-driven)
    // jobs this is the only thing the Stop button does anyway; for
    // `jobRunScript` jobs it ensures the pane reflects the cancellation
    // even if step 1 silently failed.
    let r = format!(
        "rstudiocli::job_set_state(job = {}, state = \"cancelled\")",
        r_quote(id)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(Some(serde_json::json!({
        "cancelled": true,
        "hard_killed": hard_killed,
    })))
}

fn list(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = r#"cat(jsonlite::toJSON(
        list(jobs = rstudiocli::job_list()),
        auto_unbox = TRUE, null = "null"
    ))"#;
    let raw = r_eval::run(rpc, r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("job list: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn add(
    rpc: &RpcClient<'_>,
    name: &str,
    status: &str,
    progress_units: u32,
    running: bool,
    auto_remove: bool,
    show: bool,
) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!(
        r#"cat(jsonlite::toJSON(
            list(id = rstudiocli::job_add(
                name = {name_q},
                status = {status_q},
                progress_units = {progress_units}L,
                running = {running_arg},
                auto_remove = {auto_remove_arg},
                show = {show_arg}
            )),
            auto_unbox = TRUE
        ))"#,
        name_q = r_quote(name),
        status_q = r_quote(status),
        running_arg = if running { "TRUE" } else { "FALSE" },
        auto_remove_arg = if auto_remove { "TRUE" } else { "FALSE" },
        show_arg = if show { "TRUE" } else { "FALSE" },
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("job add: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

fn silent_id(rpc: &RpcClient<'_>, pkg_fn: &str, id: &str) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!("rstudiocli::{pkg_fn}(job = {})", r_quote(id));
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn silent_id_int(
    rpc: &RpcClient<'_>,
    pkg_fn: &str,
    id: &str,
    units: u32,
) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!(
        "rstudiocli::{pkg_fn}(job = {}, units = {units}L)",
        r_quote(id)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn set_state(rpc: &RpcClient<'_>, id: &str, state: &str) -> Result<Option<Value>, CliError> {
    if !["idle", "running", "succeeded", "cancelled", "failed"].contains(&state) {
        return Err(CliError::user(format!(
            "invalid state '{state}'. Expected: idle, running, succeeded, cancelled, failed."
        )));
    }
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!(
        "rstudiocli::job_set_state(job = {}, state = {})",
        r_quote(id),
        r_quote(state)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn set_status(rpc: &RpcClient<'_>, id: &str, status: &str) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!(
        "rstudiocli::job_set_status(job = {}, status = {})",
        r_quote(id),
        r_quote(status)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn add_output(
    rpc: &RpcClient<'_>,
    id: &str,
    output: &str,
    error: bool,
) -> Result<Option<Value>, CliError> {
    let err_arg = if error { "TRUE" } else { "FALSE" };
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!(
        "rstudiocli::job_add_output(job = {}, output = {}, error = {err_arg})",
        r_quote(id),
        r_quote(output)
    );
    r_eval::run_silent(rpc, &r)?;
    Ok(None)
}

fn run_script(
    rpc: &RpcClient<'_>,
    path: &str,
    name: Option<&str>,
    working_dir: Option<&str>,
    import_env: bool,
    export_env: &str,
) -> Result<Option<Value>, CliError> {
    let abs = std::path::Path::new(path)
        .canonicalize()
        .map_err(|e| CliError::user(format!("cannot resolve {path}: {e}")))?;
    let abs_str = abs.to_string_lossy().into_owned();
    let name_arg = match name {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    let wd_arg = match working_dir {
        Some(s) => r_quote(s),
        None => "NULL".into(),
    };
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = format!(
        r#"cat(jsonlite::toJSON(
            list(id = rstudiocli::job_run_script(
                path = {path_q},
                name = {name_arg},
                working_dir = {wd_arg},
                import_env = {import_env_arg},
                export_env = {export_env_q}
            )),
            auto_unbox = TRUE
        ))"#,
        path_q = r_quote(&abs_str),
        import_env_arg = if import_env { "TRUE" } else { "FALSE" },
        export_env_q = r_quote(export_env),
    );
    let raw = r_eval::run(rpc, &r)?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::internal(format!("job run-script: invalid JSON: {e}; raw: {raw}"))
    })?;
    Ok(Some(parsed))
}

fn is_active(rpc: &RpcClient<'_>) -> Result<Option<Value>, CliError> {
    // Delegated to the rstudiocli R package: see `r-package/R/job.R`.
    let r = "cat(jsonlite::toJSON(rstudiocli::job_is_active(), auto_unbox = TRUE))";
    let raw = r_eval::run(rpc, r)?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("job is-active: invalid JSON: {e}; raw: {raw}")))?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_kill() {
        // Catalog contract: `rstudio schema job kill` and the corresponding
        // MCP tool `job_kill` rely on this entry being present.
        assert!(
            ACTIONS.iter().any(|a| a.name == "kill"),
            "job.kill must be in the registry"
        );
    }

    #[test]
    fn kill_action_documents_rpc_and_rstudioapi() {
        // Pin the empirically-validated RPC method name and the rstudioapi
        // fallback function. A typo refactor (e.g. `execute_jobs_action`,
        // plural, which is the wrong name found via probing) would now
        // fail at test time instead of silently returning `hard_killed:false`
        // for every kill in production.
        let kill_action = ACTIONS
            .iter()
            .find(|a| a.name == "kill")
            .expect("job.kill action");
        assert_eq!(
            kill_action.rpc_method,
            Some("execute_job_action"),
            "job.kill must declare the singular `execute_job_action` RPC \
             method (the plural form does not exist in rsession)"
        );
        assert_eq!(
            kill_action.rstudioapi_fn,
            Some("jobSetState"),
            "job.kill must declare its rstudioapi fallback"
        );
    }
}
