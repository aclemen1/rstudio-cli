#' List jobs in the Jobs pane
#'
#' Wraps [rstudioapi::jobList()]. Returns the named list of active job
#' entries keyed by job id, or an empty list if none are active.
#'
#' @return A named list keyed by job id. Each entry is a list of job
#'   metadata as produced by RStudio (`name`, `state`, `progress`, ...).
#' @export
job_list <- function() {
  jobs <- rstudioapi::jobList()
  if (length(jobs) == 0) list() else jobs
}

#' Register a job in the Jobs pane (manual orchestration)
#'
#' Wraps [rstudioapi::jobAdd()]. The job runs nothing on its own —
#' you drive it manually via [job_set_state()], [job_set_progress()],
#' [job_set_status()], [job_add_output()]. For "fire and forget"
#' background scripts use [job_run_script()] instead.
#'
#' @param name Job name shown in the Jobs pane.
#' @param status Initial status message.
#' @param progress_units Total units for the progress bar (0 = none).
#' @param actions Optional named list of action callbacks. See
#'   `?rstudioapi::jobAdd` for the schema.
#' @param running Initial running flag.
#' @param auto_remove Whether RStudio should auto-remove the job entry
#'   when done.
#' @param show Whether to give the Jobs pane focus.
#' @return The new job id (character).
#' @export
job_add <- function(name, status = "", progress_units = 0L,
                    actions = NULL, running = FALSE,
                    auto_remove = TRUE, show = TRUE) {
  id <- rstudioapi::jobAdd(
    name = name,
    status = status,
    progressUnits = as.integer(progress_units),
    actions = actions,
    running = running,
    autoRemove = auto_remove,
    show = show
  )
  .throttle()
  id
}

#' Remove a job from the Jobs pane
#' @param job Job id.
#' @return `NULL` invisibly. Side-effect only.
#' @export
job_remove <- function(job) {
  rstudioapi::jobRemove(job = job)
  .throttle()
  invisible(NULL)
}

#' Mark a job's progress
#' @param job Job id.
#' @param units New cumulative progress, in the units established at
#'   creation time.
#' @return `NULL` invisibly. Side-effect only.
#' @export
job_set_progress <- function(job, units) {
  rstudioapi::jobSetProgress(job = job, units = as.integer(units))
  invisible(NULL)
}

#' Bump a job's progress by `units` (incremental)
#' @param job Job id.
#' @param units Increment in the units established at creation time.
#' @return `NULL` invisibly. Side-effect only.
#' @export
job_add_progress <- function(job, units) {
  rstudioapi::jobAddProgress(job = job, units = as.integer(units))
  invisible(NULL)
}

#' Mark a job as running/stopped/finished
#' @param job Job id.
#' @param state One of `"idle"`, `"running"`, `"succeeded"`, `"cancelled"`,
#'   `"failed"`.
#' @return `NULL` invisibly. Side-effect only.
#' @export
job_set_state <- function(job, state) {
  rstudioapi::jobSetState(job = job, state = state)
  invisible(NULL)
}

#' Update the status message shown next to a job
#' @param job Job id.
#' @param status New status message.
#' @return `NULL` invisibly. Side-effect only.
#' @export
job_set_status <- function(job, status) {
  rstudioapi::jobSetStatus(job = job, status = status)
  invisible(NULL)
}

#' Append output to a job's log
#' @param job Job id.
#' @param output Output text to append.
#' @param error If `TRUE`, the output is styled as error in the pane.
#' @return `NULL` invisibly. Side-effect only.
#' @export
job_add_output <- function(job, output, error = FALSE) {
  rstudioapi::jobAddOutput(job = job, output = output, error = error)
  invisible(NULL)
}

#' Run an R script as a background job
#'
#' Wraps [rstudioapi::jobRunScript()] — RStudio spawns a separate R
#' process, runs the script, and reports lifecycle back into the Jobs
#' pane. Unlike [job_add()], this is "fire and forget" — RStudio
#' takes care of state transitions.
#'
#' @param path Path to the R script.
#' @param name Optional job name (defaults to the script's basename).
#' @param working_dir Optional working directory for the job (defaults
#'   to the script's parent).
#' @param encoding Source encoding.
#' @param import_env If `TRUE`, copy the current global env into the
#'   job process before running.
#' @param export_env Name of an env to receive the job's globalenv()
#'   on completion (empty string = don't export).
#' @return The new job id (character).
#' @export
job_run_script <- function(path, name = NULL, working_dir = NULL,
                           encoding = "unknown", import_env = FALSE,
                           export_env = "") {
  id <- rstudioapi::jobRunScript(
    path = path,
    name = name,
    workingDir = working_dir,
    encoding = encoding,
    importEnv = import_env,
    exportEnv = export_env
  )
  .throttle()
  id
}

#' Are we currently running inside a job's R process?
#'
#' Wraps [rstudioapi::isJob()]. Returns `TRUE` only when called from
#' inside a job script.
#'
#' @return A list with `is_job` (logical).
#' @export
job_is_active <- function() {
  list(is_job = rstudioapi::isJob())
}
