//! Observability mode: poll session state on a fixed interval and emit
//! one JSON Line per detected change.
//!
//! Two tiers of coverage:
//!
//! **Tier 1 (default, R-free)** — uses only the on-disk session artefacts
//! and the R-free `get_source_document` RPC. Never invokes the R
//! interpreter, so observe never competes with the user's code:
//!   - editor.opened / closed / saved / dirty / renamed
//!   - editor.typing                 (mtime of `<docid>-contents`)
//!   - console.input                 (tail `history_database`)
//!   - rsession.error                (tail rsession log)
//!   - project.changed               (`projects_settings/last-project-path`)
//!   - markers.changed               (`saved_source_markers`)
//!   - files.dir_changed             (`pcs/files-pane.pper`)
//!   - find.changed                  (`pcs/find-replace-in-files.pper`)
//!   - pane.active_column_changed    (`client-state/source-column-manager.persistent`)
//!
//! **Tier 2 (opt-in via `--with-r-state`)** — issues a single
//! `execute_r_code` per tick that returns env globals and the last R
//! error in one batch. Touches the R interpreter; will queue behind a
//! long-running computation. User opted in by passing the flag.
//!   - r.error
//!   - env.added / env.removed
//!
//! Why polling and not subscription: the CLI shares the user's clientId
//! with the browser tab (we cannot mint our own — `client_init` is
//! blacklisted to avoid resetting the user's session). Calling
//! `/events/get_events` would steal events destined for the browser.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::commands::editor::is_document_id;
use crate::error::CliError;
use crate::output::Reply;
use crate::rpc::RpcClient;
use crate::schema::{ActionSpec, ExampleSpec, ParamKind, ParamSpec};
use crate::session::Session;

const MIN_INTERVAL_S: f64 = 0.25;
const MAX_INTERVAL_S: f64 = 60.0;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        category: "observe",
        name: "events",
        summary: "Print the static catalog of event types this version emits.",
        description: "Returns a JSON document describing every event type emitted by `observe stream`, \
             with per-type tier (1 / 2 / 3), source (which file / RPC / R script populates it), \
             payload shape, whether it appears in the initial snapshot, and a one-line \
             description. Useful for agents discovering the surface, for downstream consumers \
             (parsers / validators), and for documentation. \
             \
             The catalog is static — it reflects the events this binary version COULD emit, \
             not what it has actually emitted in the current session. Combine with \
             `observe stream` for the live stream.",
        params: &[],
        examples: &[
            ExampleSpec {
                cmd: "rstudio observe events",
                explanation: "Full catalog as JSON envelope.",
            },
            ExampleSpec {
                cmd: "rstudio observe events | jq '.result.events[] | select(.tier==1) | .type'",
                explanation: "Just the event types available without R impact (Tier 1).",
            },
            ExampleSpec {
                cmd: "rstudio observe events | jq '.result.events[] | select(.type | startswith(\"env.\"))'",
                explanation: "All env.* events with their payload shape.",
            },
        ],
        returns: "{version: string, events: [{type, tier, source, description, payload, initial}]}",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: None,
    },
    ActionSpec {
        category: "observe",
        name: "stream",
        summary: "Stream session-state changes as JSON Lines on stdout (live tail).",
        description: "Polls the rsession at a configurable interval and emits one JSON \
         Line per detected change. Three coverage tiers, selected with --tier: \
         \
         Tier 1 (file-watching only, never touches R): document open / close / \
         save / dirty / typing / renamed; console.input (tail history_database); \
         rsession.error (tail rsession log); project, markers, files-pane dir, \
         find-in-files state, source-pane active column. \
         \
         Tier 2 (DEFAULT — Tier 1 + one cheap execute_r_code per tick): adds \
         r.busy_changed (latency heuristic), r.error, env.added / env.removed, \
         wd.changed, search.added / search.removed (attached packages), \
         namespaces.added / namespaces.removed (loaded namespaces). \
         \
         Tier 3 (Tier 2 + heavier introspection per tick): adds env.typed_changed \
         (class + length per global), last_value.changed (class + length of \
         .Last.value), plot.count_changed (`length(dev.list())`). \
         \
         Tier-2/3 events are buffered for up to 3 ticks waiting for the matching \
         console.input line to land in history_database. When flushed because of \
         a console.input arrival, each Tier-2/3 event is stamped with \
         `caused_by_ts_ms` pointing to the input's `rstudio_ts_ms`. When flushed \
         on timeout (3 ticks, no console activity), the field is omitted — the \
         cause was likely non-console (addin / r exec / external RPC). \
         \
         On startup, emits one event per piece of currently-known state so a \
         fresh observer sees the initial state. Output is JSONL on stdout (NOT \
         the AI-native envelope contract). SIGPIPE is reset to default so \
         `rstudio observe stream | head -n 5` exits cleanly. \
         \
         With --once, takes a single snapshot and exits — useful for scripts.",
        params: &[
            ParamSpec {
                name: "--interval",
                kind: ParamKind::Number,
                required: false,
                default: Some("1.0"),
                allowed: &[],
                description: "Polling interval in seconds. Clamped to [0.25, 60.0].",
            },
            ParamSpec {
                name: "--once",
                kind: ParamKind::Bool,
                required: false,
                default: Some("false"),
                allowed: &[],
                description: "Single snapshot then exit (no streaming loop).",
            },
            ParamSpec {
                name: "--tier",
                kind: ParamKind::Enum,
                required: false,
                default: Some("2"),
                allowed: &["1", "2", "3"],
                description: "Coverage tier. 1 = file-watching only, no R. 2 (default) = + cheap R \
                 poll (env, busy, wd, search, namespaces). 3 = + heavy introspection \
                 (typed env, last_value, plot count).",
            },
        ],
        examples: &[
            ExampleSpec {
                cmd: "rstudio observe stream",
                explanation: "Stream forever at 1 Hz, Tier 2 (default).",
            },
            ExampleSpec {
                cmd: "rstudio observe stream --tier 1",
                explanation: "Pure file-watching, zero R impact (paranoid agent / busy R session).",
            },
            ExampleSpec {
                cmd: "rstudio observe stream --tier 3 --interval 2",
                explanation: "Full introspection + slower polling to limit R impact.",
            },
            ExampleSpec {
                cmd: "rstudio observe stream | jq 'select(.type==\"console.input\")'",
                explanation: "Live stream of every console command typed by the user.",
            },
            ExampleSpec {
                cmd: "rstudio observe stream --once",
                explanation: "Single snapshot of currently-known state, no loop.",
            },
        ],
        returns: "JSON Lines stream on stdout (not the envelope contract). One line per event \
             with shape {ts: string, type: string, payload: object}. Tier-2/3 events may \
             carry `caused_by_ts_ms` referencing the rstudio_ts_ms of the triggering input.",
        errors: &[],
        rstudioapi_fn: None,
        rpc_method: Some("get_source_document"),
    },
];

/// Top-level `observe` parser. A subcommand is mandatory: `stream` for
/// the live JSONL flow, `events` for the static catalog.
#[derive(Args, Debug)]
pub struct ObserveCmd {
    #[command(subcommand)]
    pub sub: ObserveSub,
}

#[derive(Subcommand, Debug)]
pub enum ObserveSub {
    /// Stream session-state changes as JSON Lines on stdout.
    Stream(StreamArgs),
    /// Print the static catalog of event types this version emits
    /// (per-type tier, payload schema, source).
    Events,
}

#[derive(Args, Debug, Clone)]
pub struct StreamArgs {
    /// Polling interval in seconds. Clamped to [0.25, 60.0].
    #[arg(long, default_value_t = 1.0)]
    pub interval: f64,

    /// Take a single snapshot and exit (no streaming loop).
    #[arg(long)]
    pub once: bool,

    /// Coverage tier. 1 = file-watching only (no R). 2 (default) = + cheap
    /// R poll. 3 = + heavy introspection.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=3))]
    pub tier: u8,
}

pub fn run(cmd: &ObserveCmd, rpc: &RpcClient<'_>, session: &Session) -> Result<Reply, CliError> {
    match &cmd.sub {
        ObserveSub::Events => Ok(Reply::Wrapped(Some(run_events()))),
        ObserveSub::Stream(args) => run_stream(args, rpc, session),
    }
}

/// Static catalog of every event type emitted by `observe stream`.
///
/// **Maintenance**: when you add a new `emit(...)` or `pending.push(...)`
/// call site in this file, add a matching entry here. The catalog is
/// part of the public surface — agents and downstream consumers rely on
/// it for discovery and validation.
struct EventTypeSpec {
    /// Event `type` field as it appears in JSONL output (e.g. `"editor.saved"`).
    name: &'static str,
    /// Coverage tier required: 1 (file-watching), 2 (cheap R), 3 (deep R).
    tier: u8,
    /// Where the event comes from (file watched, RPC called, R script).
    source: &'static str,
    /// One-line semantic description: what triggers this event.
    description: &'static str,
    /// Lines of `name: type — description` describing the payload shape.
    payload: &'static [&'static str],
    /// True if also emitted on the initial snapshot (state seeding).
    initial: bool,
}

const EVENT_CATALOG: &[EventTypeSpec] = &[
    // ---- Tier 1: editor / sources ----
    EventTypeSpec {
        name: "editor.opened",
        tier: 1,
        source: "sources_dir + get_source_document RPC",
        description: "Document opened in the source pane (initial snapshot or new tab).",
        payload: &[
            "id: string — 8-hex document id",
            "path: string — filesystem path (empty for unsaved)",
            "dirty: bool",
            "type: string — RStudio doc type (r_source, r_markdown, ...)",
        ],
        initial: true,
    },
    EventTypeSpec {
        name: "editor.closed",
        tier: 1,
        source: "sources_dir diff",
        description: "Document closed.",
        payload: &["id: string", "path: string"],
        initial: false,
    },
    EventTypeSpec {
        name: "editor.saved",
        tier: 1,
        source: "get_source_document.last_known_write_time diff",
        description: "Document buffer saved to disk.",
        payload: &[
            "id: string",
            "path: string",
            "last_write: integer — unix epoch ms",
        ],
        initial: false,
    },
    EventTypeSpec {
        name: "editor.dirty",
        tier: 1,
        source: "get_source_document.dirty diff",
        description: "Document dirty flag flipped (unsaved edits ⇄ saved).",
        payload: &["id: string", "path: string", "dirty: bool"],
        initial: false,
    },
    EventTypeSpec {
        name: "editor.renamed",
        tier: 1,
        source: "get_source_document.path diff",
        description: "Document path changed (Save As, file move with handle kept).",
        payload: &["id: string", "from: string", "to: string"],
        initial: false,
    },
    EventTypeSpec {
        name: "editor.typing",
        tier: 1,
        source: "<docid>-contents file mtime",
        description: "Live buffer file modified — user is typing in this document.",
        payload: &["id: string", "path: string"],
        initial: false,
    },
    // ---- Tier 1: console input (history_database tail) ----
    EventTypeSpec {
        name: "console.input",
        tier: 1,
        source: "tail history_database (line per command)",
        description: "Console command submitted by the user (Enter pressed).",
        payload: &[
            "command: string — R code as typed",
            "rstudio_ts_ms: integer — RStudio-authoritative submission time, unix epoch ms",
        ],
        initial: false,
    },
    // ---- Tier 1: rsession log ----
    EventTypeSpec {
        name: "rsession.error",
        tier: 1,
        source: "tail log/rsession-<user>.log (ERROR lines)",
        description: "rsession server-side error logged (ERROR-level line).",
        payload: &["line: string — full log line including timestamp"],
        initial: false,
    },
    // ---- Tier 1: project ----
    EventTypeSpec {
        name: "project.opened",
        tier: 1,
        source: "projects_settings/last-project-path",
        description: "Active project at startup (initial snapshot only).",
        payload: &["path: string"],
        initial: true,
    },
    EventTypeSpec {
        name: "project.changed",
        tier: 1,
        source: "projects_settings/last-project-path diff",
        description: "Project switched (open / close / replace).",
        payload: &["from: string|null", "to: string|null"],
        initial: false,
    },
    // ---- Tier 1: markers / files / find / pane ----
    EventTypeSpec {
        name: "markers.active_set",
        tier: 1,
        source: "saved_source_markers JSON .active_set",
        description: "Currently displayed Markers pane set (initial only).",
        payload: &["name: string"],
        initial: true,
    },
    EventTypeSpec {
        name: "markers.changed",
        tier: 1,
        source: "saved_source_markers JSON diff",
        description: "Markers active set changed (new lint pass, manual switch, ...).",
        payload: &["from: string|null", "to: string|null"],
        initial: false,
    },
    EventTypeSpec {
        name: "files.dir_changed",
        tier: 1,
        source: "pcs/files-pane.pper JSON .path",
        description: "Files pane navigated to a different directory.",
        payload: &[
            "from: string|null",
            "to: string|null",
            "path: string (initial)",
        ],
        initial: true,
    },
    EventTypeSpec {
        name: "find.state",
        tier: 1,
        source: "pcs/find-replace-in-files.pper",
        description: "Current Find-in-Files state (initial only).",
        payload: &["query: string", "path: string"],
        initial: true,
    },
    EventTypeSpec {
        name: "find.changed",
        tier: 1,
        source: "pcs/find-replace-in-files.pper diff",
        description: "New Find-in-Files search query.",
        payload: &["query: string", "path: string"],
        initial: false,
    },
    EventTypeSpec {
        name: "pane.active_column",
        tier: 1,
        source: "client-state/source-column-manager.persistent",
        description: "Active source-pane column at startup (initial only).",
        payload: &["column: string"],
        initial: true,
    },
    EventTypeSpec {
        name: "pane.active_column_changed",
        tier: 1,
        source: "client-state/source-column-manager.persistent diff",
        description: "User switched source-pane column (multi-column layouts).",
        payload: &["from: string|null", "to: string|null"],
        initial: false,
    },
    // ---- Tier 2: R poll ----
    EventTypeSpec {
        name: "r.busy_changed",
        tier: 2,
        source: "execute_r_code latency heuristic (> 800 ms = busy)",
        description: "R interpreter transitioned between idle and busy.",
        payload: &[
            "busy: bool",
            "elapsed_ms: integer — duration of the probe call that detected the state",
        ],
        initial: false,
    },
    EventTypeSpec {
        name: "r.error",
        tier: 2,
        source: "geterrmessage() diff",
        description: "New R-level error captured by `geterrmessage()`.",
        payload: &[
            "message: string",
            "caused_by_ts_ms: integer (optional) — rstudio_ts_ms of the triggering console.input",
        ],
        initial: false,
    },
    EventTypeSpec {
        name: "env.added",
        tier: 2,
        source: "ls(envir=.GlobalEnv) diff",
        description: "New variable in `.GlobalEnv`.",
        payload: &["name: string", "caused_by_ts_ms: integer (optional)"],
        initial: true,
    },
    EventTypeSpec {
        name: "env.removed",
        tier: 2,
        source: "ls(envir=.GlobalEnv) diff",
        description: "Variable removed from `.GlobalEnv` (rm, reassignment to NULL via list, ...).",
        payload: &["name: string", "caused_by_ts_ms: integer (optional)"],
        initial: false,
    },
    EventTypeSpec {
        name: "wd.changed",
        tier: 2,
        source: "getwd() diff",
        description: "R working directory changed.",
        payload: &[
            "from: string",
            "to: string",
            "caused_by_ts_ms: integer (optional)",
        ],
        initial: true,
    },
    EventTypeSpec {
        name: "search.added",
        tier: 2,
        source: "search() diff",
        description: "Item added to the R search path (library / attach / ...).",
        payload: &["name: string", "caused_by_ts_ms: integer (optional)"],
        initial: true,
    },
    EventTypeSpec {
        name: "search.removed",
        tier: 2,
        source: "search() diff",
        description: "Item removed from the R search path (detach / unloadNamespace).",
        payload: &["name: string", "caused_by_ts_ms: integer (optional)"],
        initial: false,
    },
    EventTypeSpec {
        name: "namespaces.added",
        tier: 2,
        source: "loadedNamespaces() diff",
        description: "Namespace loaded (library, requireNamespace, transitive load).",
        payload: &["name: string", "caused_by_ts_ms: integer (optional)"],
        initial: true,
    },
    EventTypeSpec {
        name: "namespaces.removed",
        tier: 2,
        source: "loadedNamespaces() diff",
        description: "Namespace unloaded.",
        payload: &["name: string", "caused_by_ts_ms: integer (optional)"],
        initial: false,
    },
    // ---- Tier 3: deep introspection ----
    EventTypeSpec {
        name: "env.typed_changed",
        tier: 3,
        source: "lapply(.GlobalEnv, ...) per-name class + length diff",
        description: "A `.GlobalEnv` variable's class or length changed (reassigned with different shape).",
        payload: &[
            "name: string",
            "class: string — class()[1]",
            "length: integer",
            "from: {class, length} — previous shape (omitted on first sight)",
            "caused_by_ts_ms: integer (optional)",
        ],
        initial: true,
    },
    EventTypeSpec {
        name: "last_value.changed",
        tier: 3,
        source: ".Last.value class + length diff",
        description: "REPL's `.Last.value` summary changed (a new top-level expression was evaluated).",
        payload: &[
            "class: string",
            "length: integer",
            "caused_by_ts_ms: integer (optional)",
        ],
        initial: true,
    },
    EventTypeSpec {
        name: "plot.count_changed",
        tier: 3,
        source: "length(grDevices::dev.list())",
        description: "Number of open graphics devices changed (new plot rendered, dev.off called).",
        payload: &[
            "from: integer",
            "to: integer",
            "caused_by_ts_ms: integer (optional)",
        ],
        initial: true,
    },
    // ---- All tiers: internal ----
    EventTypeSpec {
        name: "session.error",
        tier: 1,
        source: "internal — emitted when the polling loop hits a recoverable error",
        description: "A tick-level failure (RPC unavailable, R-state probe crashed, ...). Loop continues.",
        payload: &["message: string"],
        initial: false,
    },
];

fn run_events() -> Value {
    let events: Vec<Value> = EVENT_CATALOG
        .iter()
        .map(|e| {
            json!({
                "type": e.name,
                "tier": e.tier,
                "source": e.source,
                "description": e.description,
                "payload": e.payload,
                "initial": e.initial,
            })
        })
        .collect();
    json!({
        "version": crate::VERSION,
        "count": events.len(),
        "events": events,
    })
}

fn run_stream(
    args: &StreamArgs,
    rpc: &RpcClient<'_>,
    session: &Session,
) -> Result<Reply, CliError> {
    // Reset SIGPIPE to default so `rstudio observe stream | head -n 5` terminates
    // silently when the consumer closes stdout, rather than panicking on
    // EPIPE.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let interval_s = args.interval.clamp(MIN_INTERVAL_S, MAX_INTERVAL_S);
    let dur = Duration::from_secs_f64(interval_s);
    let tier = args.tier;
    let paths = DiskPaths::resolve(session)?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Initial snapshot: emit one event per piece of currently-known state
    // so a fresh observer sees the initial picture. Tail offsets are seeded
    // from the current end-of-file (we don't replay history).
    let docs = take_documents(rpc, session)?;
    for doc in docs.values() {
        emit(&mut out, "editor.opened", doc.to_payload())?;
    }
    let mut state = WatcherState {
        history_offset: file_size(&paths.history_database),
        log_offset: file_size_opt(paths.log.as_deref()),
        documents: docs,
        contents_mtimes: take_contents_mtimes(session)?,
        ..Default::default()
    };

    if let Some(p) = read_text_file(&paths.project_path_file) {
        emit(&mut out, "project.opened", json!({ "path": p }))?;
        state.project_path = Some(p);
    }
    if let Some(d) = read_files_pane_dir(&paths.files_pane) {
        emit(&mut out, "files.dir_changed", json!({ "path": d }))?;
        state.files_dir = Some(d);
    }
    if let Some((q, p)) = read_find_state(&paths.find_state) {
        emit(&mut out, "find.state", json!({ "query": q, "path": p }))?;
        state.find_query = Some(q);
    }
    if let Some(set_name) = read_active_marker_set(&paths.markers) {
        emit(&mut out, "markers.active_set", json!({ "name": set_name }))?;
        state.markers_active_set = Some(set_name);
    }
    if let Some(col) = read_active_column(&paths.column_state) {
        emit(&mut out, "pane.active_column", json!({ "column": col }))?;
        state.column_active = Some(col);
    }
    if tier >= 2 {
        match take_r_state(rpc, tier) {
            Ok(rs) => {
                // Skip emitting `r.error` on initial snapshot: geterrmessage()
                // is sticky and can hold a long-stale message. Errors will be
                // reported on tick when they actually change. Other current
                // state IS emitted so the agent sees the initial picture.
                if !rs.wd.is_empty() {
                    emit(&mut out, "wd.changed", json!({ "to": rs.wd }))?;
                }
                for s in &rs.search {
                    emit(&mut out, "search.added", json!({ "name": s }))?;
                }
                for n in &rs.namespaces {
                    emit(&mut out, "namespaces.added", json!({ "name": n }))?;
                }
                for g in &rs.globals {
                    emit(&mut out, "env.added", json!({ "name": g }))?;
                }
                if tier >= 3 {
                    for (name, summary) in &rs.globals_typed {
                        emit(
                            &mut out,
                            "env.typed_changed",
                            json!({ "name": name, "class": summary.class,
                                    "length": summary.length }),
                        )?;
                    }
                    if let Some(s) = &rs.last_value
                        && !s.class.is_empty()
                    {
                        emit(
                            &mut out,
                            "last_value.changed",
                            json!({ "class": s.class, "length": s.length }),
                        )?;
                    }
                    if rs.plot_count > 0 {
                        emit(
                            &mut out,
                            "plot.count_changed",
                            json!({ "to": rs.plot_count }),
                        )?;
                    }
                }
                state.was_busy = rs.elapsed_ms > BUSY_LATENCY_THRESHOLD_MS;
                state.r_state = Some(rs);
            }
            Err(e) => emit(
                &mut out,
                "session.error",
                json!({ "message": format!("could not read R state: {e}") }),
            )?,
        }
    }
    out.flush().ok();

    if args.once {
        std::process::exit(0);
    }

    loop {
        thread::sleep(dur);
        if let Err(e) = tick(rpc, session, &paths, &mut state, &mut out, tier) {
            emit(
                &mut out,
                "session.error",
                json!({ "message": e.to_string() }),
            )?;
            out.flush().ok();
        }
    }
}

fn tick<W: Write>(
    rpc: &RpcClient<'_>,
    session: &Session,
    paths: &DiskPaths,
    state: &mut WatcherState,
    out: &mut W,
    tier: u8,
) -> Result<(), CliError> {
    // Documents (open/closed/dirty/renamed/saved).
    let docs = take_documents(rpc, session)?;
    diff_documents(out, &state.documents, &docs)?;
    state.documents = docs;

    // Per-document live-buffer mtime → editor.typing.
    let mtimes = take_contents_mtimes(session)?;
    diff_contents_mtimes(out, &state.contents_mtimes, &mtimes, &state.documents)?;
    state.contents_mtimes = mtimes;

    // Tail history_database → console.input. Track whether new lines
    // appeared this tick, and the rstudio_ts_ms of the last one, so that
    // a flush of pending Tier-2 events can stamp them with caused_by_ts_ms.
    let new_history = read_new_lines(&paths.history_database, &mut state.history_offset);
    let history_emitted_this_tick = !new_history.is_empty();
    let mut last_console_ts_ms: Option<u64> = None;
    for line in new_history {
        if let Some((ts_ms, cmd)) = parse_history_line(&line) {
            last_console_ts_ms = Some(ts_ms);
            emit(
                out,
                "console.input",
                json!({ "rstudio_ts_ms": ts_ms, "command": cmd }),
            )?;
        }
    }

    // Tail rsession log → rsession.error (filter ERROR lines only).
    if let Some(log) = paths.log.as_deref() {
        let new_log = read_new_lines(log, &mut state.log_offset);
        for line in new_log {
            if line.contains(" ERROR ") {
                emit(out, "rsession.error", json!({ "line": line }))?;
            }
        }
    }

    // project / files / find / markers / pane.column.
    let project = read_text_file(&paths.project_path_file);
    if project != state.project_path {
        emit(
            out,
            "project.changed",
            json!({ "from": state.project_path, "to": project }),
        )?;
        state.project_path = project;
    }

    let files_dir = read_files_pane_dir(&paths.files_pane);
    if files_dir != state.files_dir {
        emit(
            out,
            "files.dir_changed",
            json!({ "from": state.files_dir, "to": files_dir }),
        )?;
        state.files_dir = files_dir;
    }

    let find = read_find_state(&paths.find_state);
    let find_query = find.as_ref().map(|(q, _)| q.clone());
    if find_query != state.find_query
        && let Some((q, p)) = find
    {
        emit(out, "find.changed", json!({ "query": q, "path": p }))?;
        state.find_query = Some(q);
    }

    let markers = read_active_marker_set(&paths.markers);
    if markers != state.markers_active_set {
        emit(
            out,
            "markers.changed",
            json!({ "from": state.markers_active_set, "to": markers }),
        )?;
        state.markers_active_set = markers;
    }

    let column = read_active_column(&paths.column_state);
    if column != state.column_active {
        emit(
            out,
            "pane.active_column_changed",
            json!({ "from": state.column_active, "to": column }),
        )?;
        state.column_active = column;
    }

    // Tier 2 / Tier 3: R state. Hold events in a buffer until either
    // the corresponding history_database lines catch up OR the hold
    // timer expires — see MAX_PENDING_TICKS. r.busy_changed is emitted
    // immediately (not buffered): the busy signal is a high-priority
    // status update where ordering with console.input doesn't help.
    if tier >= 2 {
        match take_r_state(rpc, tier) {
            Ok(rs) => {
                let is_busy = rs.elapsed_ms > BUSY_LATENCY_THRESHOLD_MS;
                if is_busy != state.was_busy {
                    emit(
                        out,
                        "r.busy_changed",
                        json!({ "busy": is_busy, "elapsed_ms": rs.elapsed_ms }),
                    )?;
                    state.was_busy = is_busy;
                }

                let prev = state.r_state.take().unwrap_or_default();
                diff_r_state(&prev, &rs, tier, &mut state.pending_r);
                state.r_state = Some(rs);
            }
            Err(e) => emit(
                out,
                "session.error",
                json!({ "message": format!("could not read R state: {e}") }),
            )?,
        }

        if !state.pending_r.is_empty() {
            state.pending_r.held_for += 1;
            let should_flush =
                history_emitted_this_tick || state.pending_r.held_for >= MAX_PENDING_TICKS;
            if should_flush {
                // If we're flushing because a console.input just landed,
                // we have a strong correlation key: the cause was the
                // last command typed. Stamp it. On timeout (no console
                // activity), the cause is unknown — non-console activity
                // (addin / r exec / external RPC).
                let cause = if history_emitted_this_tick {
                    last_console_ts_ms
                } else {
                    None
                };
                let events = std::mem::take(&mut state.pending_r.events);
                for (kind, payload) in events {
                    emit(out, &kind, with_cause(payload, cause))?;
                }
                state.pending_r.held_for = 0;
            }
        }
    }

    out.flush().ok();
    Ok(())
}

#[derive(Default, Debug)]
struct WatcherState {
    documents: BTreeMap<String, DocSnapshot>,
    contents_mtimes: BTreeMap<String, SystemTime>,
    history_offset: u64,
    log_offset: u64,
    project_path: Option<String>,
    files_dir: Option<String>,
    find_query: Option<String>,
    markers_active_set: Option<String>,
    column_active: Option<String>,
    r_state: Option<RStateSnapshot>,
    was_busy: bool,
    /// Tier-2/3 events held until the corresponding `console.input` lines
    /// catch up in `history_database`. RStudio writes the history file
    /// AFTER R has finished executing, so without buffering we'd emit the
    /// effect (env.added) before the cause (console.input). Flushed when
    /// either history catches up OR the buffer has been held for
    /// MAX_PENDING_TICKS, whichever comes first.
    pending_r: PendingRChanges,
}

#[derive(Default, Debug)]
struct PendingRChanges {
    /// One pending event = (kind, payload). Order is preserved: events
    /// are emitted in the order they were buffered, which matches the
    /// order they appeared in the take_r_state diff phase.
    events: Vec<(String, Value)>,
    held_for: u32,
}

impl PendingRChanges {
    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    fn push(&mut self, kind: &str, payload: Value) {
        self.events.push((kind.to_string(), payload));
    }
}

/// How many ticks to hold Tier-2 events while waiting for the matching
/// `console.input` to land in `history_database`. Beyond this, we flush
/// anyway — R-state changes triggered by non-console activity (e.g.
/// `rstudio r exec`, RStudio addins) never appear in history_database.
const MAX_PENDING_TICKS: u32 = 3;

#[derive(Debug, Clone)]
struct DocSnapshot {
    id: String,
    path: String,
    dirty: bool,
    last_write: i64,
    doc_type: String,
}

impl DocSnapshot {
    fn to_payload(&self) -> Value {
        json!({
            "id": self.id,
            "path": self.path,
            "dirty": self.dirty,
            "type": self.doc_type,
        })
    }
}

#[derive(Default, Debug, Clone)]
struct RStateSnapshot {
    // Tier 2 fields (always populated when --tier >= 2)
    error: String,
    globals: BTreeSet<String>,
    wd: String,
    search: BTreeSet<String>,
    namespaces: BTreeSet<String>,
    /// Wall-clock duration of the take_r_state RPC in milliseconds.
    /// Used as a heuristic for r.busy_changed: if the call took longer
    /// than BUSY_LATENCY_THRESHOLD_MS, R was busy and we waited.
    elapsed_ms: u128,
    // Tier 3 fields (populated only when --tier >= 3)
    globals_typed: BTreeMap<String, GlobalSummary>,
    last_value: Option<GlobalSummary>,
    plot_count: u32,
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct GlobalSummary {
    class: String,
    length: i64,
}

/// Threshold (ms) above which we consider R was busy during the call.
/// Calibrated to be well above typical idle-call cost (5-50ms) and below
/// human-perceptible "responsive" feel (~1s).
const BUSY_LATENCY_THRESHOLD_MS: u128 = 800;

/// Disk paths used by the file-watching tier. Resolved at startup; if any
/// optional path is absent (e.g. no rsession log, fresh session), the
/// corresponding watcher silently no-ops.
struct DiskPaths {
    history_database: PathBuf,
    log: Option<PathBuf>,
    project_path_file: PathBuf,
    markers: PathBuf,
    files_pane: PathBuf,
    find_state: PathBuf,
    column_state: PathBuf,
}

impl DiskPaths {
    fn resolve(_session: &Session) -> Result<Self, CliError> {
        // The global RStudio data root (`~/.local/share/rstudio`) is fixed
        // regardless of which project is open. Only the per-session sources
        // directory relocates inside `<project>/.Rproj.user/...` when a
        // project is active — that is handled by `Session::resolve_sources_dir`.
        // All the watcher targets in DiskPaths live at the global root.
        let home = std::env::var("HOME").map_err(|e| {
            CliError::session(format!("cannot read HOME for rstudio data root: {e}"))
        })?;
        let rstudio_root = PathBuf::from(home).join(".local/share/rstudio");
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());

        Ok(Self {
            history_database: rstudio_root.join("history_database"),
            log: {
                let p = rstudio_root
                    .join("log")
                    .join(format!("rsession-{user}.log"));
                if p.exists() { Some(p) } else { None }
            },
            project_path_file: rstudio_root
                .join("projects_settings")
                .join("last-project-path"),
            markers: rstudio_root.join("saved_source_markers"),
            files_pane: rstudio_root.join("pcs").join("files-pane.pper"),
            find_state: rstudio_root.join("pcs").join("find-replace-in-files.pper"),
            column_state: rstudio_root
                .join("client-state")
                .join("source-column-manager.persistent"),
        })
    }
}

fn take_documents(
    rpc: &RpcClient<'_>,
    session: &Session,
) -> Result<BTreeMap<String, DocSnapshot>, CliError> {
    let dir = session.resolve_sources_dir()?;
    let entries = fs::read_dir(&dir).map_err(|e| {
        CliError::session(format!(
            "cannot read RStudio sources directory {}: {e}",
            dir.display()
        ))
    })?;

    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_document_id(&name) {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    ids.sort();

    let mut map = BTreeMap::new();
    for id in ids {
        let result = rpc.rpc("get_source_document", vec![Value::String(id.clone())]);
        if let Ok(Value::Object(m)) = result {
            map.insert(
                id.clone(),
                DocSnapshot {
                    id,
                    path: m
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    dirty: m.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false),
                    last_write: m
                        .get("last_known_write_time")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    doc_type: m
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
            );
        }
    }
    Ok(map)
}

/// Per-document live-buffer mtime. Used to detect typing without RPC.
fn take_contents_mtimes(session: &Session) -> Result<BTreeMap<String, SystemTime>, CliError> {
    let dir = session.resolve_sources_dir()?;
    let mut map = BTreeMap::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(map),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Live-buffer companion files have shape "<8-hex>-contents".
        if let Some(id) = name.strip_suffix("-contents")
            && is_document_id(id)
            && let Ok(meta) = entry.metadata()
            && let Ok(mtime) = meta.modified()
        {
            map.insert(id.to_string(), mtime);
        }
    }
    Ok(map)
}

fn diff_documents<W: Write>(
    out: &mut W,
    prev: &BTreeMap<String, DocSnapshot>,
    curr: &BTreeMap<String, DocSnapshot>,
) -> Result<(), CliError> {
    for (id, doc) in prev {
        if !curr.contains_key(id) {
            emit(out, "editor.closed", json!({ "id": id, "path": doc.path }))?;
        }
    }
    for (id, doc) in curr {
        match prev.get(id) {
            None => emit(out, "editor.opened", doc.to_payload())?,
            Some(p) => {
                if p.path != doc.path {
                    emit(
                        out,
                        "editor.renamed",
                        json!({ "id": id, "from": p.path, "to": doc.path }),
                    )?;
                }
                if p.dirty != doc.dirty {
                    emit(
                        out,
                        "editor.dirty",
                        json!({ "id": id, "path": doc.path, "dirty": doc.dirty }),
                    )?;
                }
                if p.last_write != doc.last_write && p.last_write > 0 && doc.last_write > 0 {
                    emit(
                        out,
                        "editor.saved",
                        json!({ "id": id, "path": doc.path, "last_write": doc.last_write }),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn diff_contents_mtimes<W: Write>(
    out: &mut W,
    prev: &BTreeMap<String, SystemTime>,
    curr: &BTreeMap<String, SystemTime>,
    docs: &BTreeMap<String, DocSnapshot>,
) -> Result<(), CliError> {
    for (id, mtime) in curr {
        if prev.get(id) != Some(mtime) {
            // Only emit for docs we know about; skip detached buffers.
            let path = docs.get(id).map(|d| d.path.clone()).unwrap_or_default();
            emit(out, "editor.typing", json!({ "id": id, "path": path }))?;
        }
    }
    Ok(())
}

/// Read the appended bytes since `*offset` and return them split by '\n'.
/// Updates `*offset` to the new file size. Lines that are short enough to
/// be a partial last line (no trailing '\n') are skipped — they'll be
/// picked up next tick once complete.
fn read_new_lines(path: &Path, offset: &mut u64) -> Vec<String> {
    let mut out = Vec::new();
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return out,
    };
    let size = file.metadata().ok().map(|m| m.len()).unwrap_or(0);
    if size == *offset {
        return out;
    }
    if size < *offset {
        // File was truncated / rotated. Reset.
        *offset = size;
        return out;
    }
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return out;
    }
    let mut buf = Vec::with_capacity((size - *offset) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return out;
    }
    let s = String::from_utf8_lossy(&buf);
    let mut last_nl_pos: usize = 0;
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 || s.starts_with('\n') {
            // not the trailing partial; the previous newline ended the prior line.
        }
        if let Some(stripped) = line.strip_suffix('\r') {
            out.push(stripped.to_string());
        } else {
            // For the very last segment, if the source did not end with '\n',
            // it's a partial line — skip it; we'll re-read on next tick.
            // Detect partial: find absolute byte position of `line` in `s`.
            let line_end = last_nl_pos + line.len();
            last_nl_pos = line_end + 1;
            if line_end == s.len() && !s.ends_with('\n') {
                // partial; rewind offset so we re-read next tick.
                *offset = size - line.len() as u64;
                continue;
            }
            if !line.is_empty() {
                out.push(line.to_string());
            }
        }
    }
    if s.ends_with('\n') {
        *offset = size;
    }
    out
}

fn parse_history_line(line: &str) -> Option<(u64, String)> {
    let (ts, cmd) = line.split_once(':')?;
    let ts_ms: u64 = ts.parse().ok()?;
    Some((ts_ms, cmd.to_string()))
}

fn read_text_file(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn read_files_pane_dir(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v.get("path")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn read_find_state(path: &Path) -> Option<(String, String)> {
    let raw = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let dialog = v.get("dialog-state")?;
    let q = dialog.get("query")?.as_str()?.to_string();
    let p = dialog
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    Some((q, p))
}

fn read_active_marker_set(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v.get("active_set")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn read_active_column(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v.get("column-info")
        .and_then(|c| c.get("activeColumn"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn file_size_opt(path: Option<&Path>) -> u64 {
    path.map(file_size).unwrap_or(0)
}

/// One execute_r_code per tick. The script body grows with the tier.
/// Returns the parsed snapshot plus the call's wall-clock duration —
/// used as the busy heuristic.
fn take_r_state(rpc: &RpcClient<'_>, tier: u8) -> Result<RStateSnapshot, CliError> {
    let r_code = r_state_script(tier);
    let start = std::time::Instant::now();
    let raw = crate::r_eval::run(rpc, &r_code)?;
    let elapsed_ms = start.elapsed().as_millis();
    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::internal(format!("observe: r_state JSON: {e}; raw: {raw}")))?;

    let error = v
        .get("err")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let globals: BTreeSet<String> = string_array(&v, "globals");
    let wd = v
        .get("wd")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let search: BTreeSet<String> = string_array(&v, "search");
    let namespaces: BTreeSet<String> = string_array(&v, "namespaces");

    let mut snap = RStateSnapshot {
        error,
        globals,
        wd,
        search,
        namespaces,
        elapsed_ms,
        ..Default::default()
    };

    if tier >= 3 {
        if let Some(typed) = v.get("globals_typed").and_then(|x| x.as_object()) {
            for (name, summary) in typed {
                if let Some(s) = parse_summary(summary) {
                    snap.globals_typed.insert(name.clone(), s);
                }
            }
        }
        snap.last_value = v.get("last_value").and_then(parse_summary);
        snap.plot_count = v.get("plot_count").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    }
    Ok(snap)
}

fn string_array(v: &Value, key: &str) -> BTreeSet<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_summary(v: &Value) -> Option<GlobalSummary> {
    let obj = v.as_object()?;
    Some(GlobalSummary {
        class: obj
            .get("class")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        length: obj.get("length").and_then(|x| x.as_i64()).unwrap_or(0),
    })
}

fn r_state_script(tier: u8) -> String {
    let mut parts: Vec<&str> = vec![
        "err = jsonlite::unbox(tryCatch(geterrmessage(), error = function(e) ''))",
        "globals = ls(envir = .GlobalEnv)",
        "wd = jsonlite::unbox(getwd())",
        "search = search()",
        "namespaces = loadedNamespaces()",
    ];
    let tier3 = if tier >= 3 {
        Some(
            "globals_typed = lapply(setNames(nm = ls(envir = .GlobalEnv)), function(n) { \
                v <- get(n, envir = .GlobalEnv); \
                list(class = jsonlite::unbox(class(v)[1]), \
                     length = jsonlite::unbox(as.integer(length(v)))) }), \
             last_value = if (exists('.Last.value', envir = baseenv())) \
                list(class = jsonlite::unbox(class(.Last.value)[1]), \
                     length = jsonlite::unbox(as.integer(length(.Last.value)))) \
              else list(class = jsonlite::unbox(''), length = jsonlite::unbox(0L)), \
             plot_count = jsonlite::unbox(length(grDevices::dev.list()))",
        )
    } else {
        None
    };
    if let Some(t3) = tier3 {
        parts.push(t3);
    }
    format!(
        "cat(jsonlite::toJSON(list({}), auto_unbox = FALSE))",
        parts.join(", ")
    )
}

/// Compute the diff between two R-state snapshots and append the
/// resulting events to the pending buffer. Tier 2 covers globals (just
/// names), error, wd, search, namespaces. Tier 3 additionally covers
/// globals_typed, last_value, plot_count.
fn diff_r_state(
    prev: &RStateSnapshot,
    curr: &RStateSnapshot,
    tier: u8,
    pending: &mut PendingRChanges,
) {
    // r.error
    if curr.error != prev.error && !curr.error.is_empty() {
        pending.push("r.error", json!({ "message": curr.error }));
    }
    // env names (globals).
    for g in curr.globals.difference(&prev.globals) {
        pending.push("env.added", json!({ "name": g }));
    }
    for g in prev.globals.difference(&curr.globals) {
        pending.push("env.removed", json!({ "name": g }));
    }
    // wd
    if curr.wd != prev.wd && !curr.wd.is_empty() {
        pending.push("wd.changed", json!({ "from": prev.wd, "to": curr.wd }));
    }
    // search() — attached packages / environments.
    for s in curr.search.difference(&prev.search) {
        pending.push("search.added", json!({ "name": s }));
    }
    for s in prev.search.difference(&curr.search) {
        pending.push("search.removed", json!({ "name": s }));
    }
    // loadedNamespaces() — namespaces loaded but not necessarily attached.
    for n in curr.namespaces.difference(&prev.namespaces) {
        pending.push("namespaces.added", json!({ "name": n }));
    }
    for n in prev.namespaces.difference(&curr.namespaces) {
        pending.push("namespaces.removed", json!({ "name": n }));
    }
    if tier < 3 {
        return;
    }
    // env.typed_changed: per-name, emit when class or length changed.
    for (name, summary) in &curr.globals_typed {
        match prev.globals_typed.get(name) {
            None => pending.push(
                "env.typed_changed",
                json!({ "name": name, "class": summary.class, "length": summary.length }),
            ),
            Some(p) if p != summary => pending.push(
                "env.typed_changed",
                json!({ "name": name, "class": summary.class, "length": summary.length,
                        "from": { "class": p.class, "length": p.length } }),
            ),
            _ => {}
        }
    }
    // last_value summary change.
    if let Some(s) = &curr.last_value
        && curr.last_value != prev.last_value
    {
        pending.push(
            "last_value.changed",
            json!({ "class": s.class, "length": s.length }),
        );
    }
    // plot.count_changed
    if curr.plot_count != prev.plot_count {
        pending.push(
            "plot.count_changed",
            json!({ "from": prev.plot_count, "to": curr.plot_count }),
        );
    }
}

fn with_cause(mut payload: Value, cause: Option<u64>) -> Value {
    if let Some(ms) = cause
        && let Value::Object(ref mut m) = payload
    {
        m.insert("caused_by_ts_ms".to_string(), Value::Number(ms.into()));
    }
    payload
}

fn emit<W: Write>(out: &mut W, kind: &str, payload: Value) -> Result<(), CliError> {
    let line = json!({
        "ts": iso_now(),
        "type": kind,
        "payload": payload,
    });
    writeln!(out, "{line}")
        .map_err(|e| CliError::internal(format!("observe: write to stdout: {e}")))
}

fn iso_now() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let ms = dur.subsec_millis();
    let (y, mo, d, h, mi, s) = epoch_to_components(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// Howard Hinnant's `civil_from_days`: seconds since the Unix epoch →
/// (year, month, day, hour, minute, second) in proleptic Gregorian / UTC.
fn epoch_to_components(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    const S_PER_DAY: u64 = 86_400;
    let days = (secs / S_PER_DAY) as i64;
    let s_today = secs % S_PER_DAY;
    let h = (s_today / 3600) as u32;
    let mi = ((s_today % 3600) / 60) as u32;
    let s = (s_today % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };

    (y as i32, m, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_now_shape() {
        let s = iso_now();
        assert_eq!(s.len(), 24);
        assert!(s.ends_with('Z'));
        assert_eq!(s.chars().nth(4), Some('-'));
        assert_eq!(s.chars().nth(10), Some('T'));
    }

    #[test]
    fn epoch_zero_is_1970() {
        assert_eq!(epoch_to_components(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_2000_03_01() {
        assert_eq!(epoch_to_components(951_868_800), (2000, 3, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_2024_02_29() {
        assert_eq!(
            epoch_to_components(1_709_210_096),
            (2024, 2, 29, 12, 34, 56)
        );
    }

    #[test]
    fn parses_history_line() {
        assert_eq!(
            parse_history_line("1777846335124:ls()"),
            Some((1_777_846_335_124, "ls()".into()))
        );
        assert_eq!(parse_history_line("garbage"), None);
    }

    #[test]
    fn parses_history_line_with_colon_in_command() {
        // The first ':' is the delimiter; subsequent ':' belong to the command.
        assert_eq!(
            parse_history_line("1234:strsplit('a:b', ':')"),
            Some((1234, "strsplit('a:b', ':')".into()))
        );
    }
}
