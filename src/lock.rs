//! Session-scoped advisory file lock for cross-CLI-invocation
//! coordination of write commands.
//!
//! Each session has a single exclusive flock at
//! `~/.config/rstudio-cli/locks/session-<id>.lock`. Write commands
//! acquire it for the duration of one call (Phase 1: per-call mutex);
//! `rstudio tx -- <cmd>` holds it across a child process so that
//! every `rstudio` invocation inside the child runs atomically with
//! respect to other agents (Phase 2: multi-call atomicity via
//! fork-inherit, à la `flock(1)` from util-linux).
//!
//! Why flock + fork-inherit: the kernel releases the lock when the
//! holding process exits — clean even on `kill -9`, no stale locks,
//! no PID files, no heartbeat, no daemon. The same `lock.rs` powers
//! both phases; the only thing that changes is the lifetime of the
//! holding process.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::error::CliError;

/// Env var set by `rstudio tx -- <cmd>` for child `rstudio`
/// invocations to know they're already inside an outer tx — they
/// MUST skip acquisition or they would deadlock (the parent already
/// holds the lock via a different open file description).
pub const TX_ENV: &str = "RSTUDIO_TX_HELD";

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub struct SessionLock {
    /// Open file descriptor. Drop closes the FD; the kernel releases
    /// the flock atomically. We never call `flock(LOCK_UN)` ourselves.
    _file: File,
    sidecar: PathBuf,
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Best-effort sidecar cleanup. If this fails (e.g. file already
        // gone), we ignore it; the next holder will overwrite anyway.
        let _ = fs::remove_file(&self.sidecar);
    }
}

impl SessionLock {
    /// Acquire the session lock, blocking up to `timeout`.
    ///
    /// `command` is recorded in the sidecar `session-<id>.holder.json`
    /// so that competing agents who time out can see who is holding
    /// the lock and what they're doing.
    ///
    /// Idempotent under `RSTUDIO_TX_HELD`: callers should check
    /// `inside_tx()` before calling this. We don't auto-skip here
    /// because the bypass decision is policy (does this command type
    /// need a lock?) rather than mechanism.
    pub fn acquire(session_id: &str, timeout: Duration, command: &str) -> Result<Self, CliError> {
        let lock_dir = lock_dir()?;
        fs::create_dir_all(&lock_dir).map_err(|e| {
            CliError::internal(format!("lock: create dir {}: {e}", lock_dir.display()))
        })?;
        let lock_path = lock_dir.join(format!("session-{session_id}.lock"));
        let sidecar = lock_dir.join(format!("session-{session_id}.holder.json"));

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| CliError::internal(format!("lock: open {}: {e}", lock_path.display())))?;

        let fd = file.as_raw_fd();
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: `fd` is valid for the lifetime of `file`. flock(2)
            // takes any value; invalid fds return EBADF which we surface.
            let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                break;
            }
            let err = std::io::Error::last_os_error();
            // EWOULDBLOCK means held by someone else. Anything else is a
            // real error worth surfacing.
            if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(CliError::internal(format!("lock: flock failed: {err}")));
            }
            if Instant::now() >= deadline {
                let holder = read_sidecar(&sidecar);
                return Err(holder_timeout_error(&lock_path, holder, timeout));
            }
            thread::sleep(POLL_INTERVAL);
        }

        let _ = write_sidecar(&sidecar, command);
        Ok(SessionLock {
            _file: file,
            sidecar,
        })
    }

    /// True if we're already inside an outer `rstudio tx --` and must
    /// skip acquisition. Cheap env lookup; safe to call from anywhere.
    pub fn inside_tx() -> bool {
        std::env::var_os(TX_ENV).is_some()
    }
}

/// Snapshot of the lock state for a given session, read without
/// acquiring. Used by `rstudio status` to report `session.lock` and
/// help agents/humans understand whether another agent is currently
/// writing to the session.
///
/// Note: this is a *moment-in-time* read. The state can change between
/// the inspect and the next operation — agents must not gate behaviour
/// on it (use `rstudio tx --` for atomicity instead). Information-only.
#[derive(Debug)]
pub struct LockState {
    pub holder: Option<LockHolder>,
}

#[derive(Debug)]
pub struct LockHolder {
    pub pid: u64,
    pub command: String,
    pub started_ms: u64,
}

pub fn inspect(session_id: &str) -> LockState {
    let dir = match lock_dir() {
        Ok(d) => d,
        Err(_) => return LockState { holder: None },
    };
    let sidecar = dir.join(format!("session-{session_id}.holder.json"));
    let v = match read_sidecar(&sidecar) {
        Some(v) => v,
        None => return LockState { holder: None },
    };
    let holder = LockHolder {
        pid: v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0),
        command: v
            .get("command")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        started_ms: v.get("started_ms").and_then(|x| x.as_u64()).unwrap_or(0),
    };
    LockState {
        holder: Some(holder),
    }
}

fn lock_dir() -> Result<PathBuf, CliError> {
    dirs::config_dir()
        .map(|p| p.join("rstudio-cli").join("locks"))
        .ok_or_else(|| {
            CliError::internal("lock: cannot determine config dir (HOME not set?)".to_string())
        })
}

fn write_sidecar(path: &Path, command: &str) -> std::io::Result<()> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let pid = std::process::id();
    let content = json!({
        "pid": pid,
        "command": command,
        "started_ms": now_ms,
    });
    let mut f = fs::File::create(path)?;
    writeln!(f, "{content}")
}

fn read_sidecar(path: &Path) -> Option<serde_json::Value> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn holder_timeout_error(
    lock_path: &Path,
    holder: Option<serde_json::Value>,
    timeout: Duration,
) -> CliError {
    let detail = match holder {
        Some(h) => format!(
            "held by pid={} command={:?} started_ms={}",
            h.get("pid").and_then(|x| x.as_u64()).unwrap_or(0),
            h.get("command").and_then(|x| x.as_str()).unwrap_or("?"),
            h.get("started_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        ),
        None => "holder unknown (no sidecar — likely a crashed prior run)".to_string(),
    };
    CliError::user(format!(
        "lock: timed out after {:.1}s waiting on {} ({}). \
         Another rstudio process is currently writing to this session. \
         Wait, retry, or pass --no-lock to bypass.",
        timeout.as_secs_f64(),
        lock_path.display(),
        detail,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Use a unique session id per test so tests don't collide on the
    /// real `~/.config/rstudio-cli/locks/`. We don't try to redirect
    /// the lock dir; the real one is fine for tests since each id is
    /// unique and we clean up sidecar via Drop.
    fn unique_id() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("test-{nanos}-{}", std::process::id())
    }

    fn cleanup(id: &str) {
        if let Ok(dir) = lock_dir() {
            let _ = fs::remove_file(dir.join(format!("session-{id}.lock")));
            let _ = fs::remove_file(dir.join(format!("session-{id}.holder.json")));
        }
    }

    #[test]
    fn acquire_release_roundtrip() {
        let id = unique_id();
        {
            let lock = SessionLock::acquire(&id, Duration::from_secs(1), "test").unwrap();
            // Sidecar exists while held.
            let sidecar = lock_dir()
                .unwrap()
                .join(format!("session-{id}.holder.json"));
            assert!(sidecar.exists(), "sidecar should exist while lock is held");
            drop(lock);
            // Sidecar cleaned up after Drop.
            assert!(
                !sidecar.exists(),
                "sidecar should be removed when lock dropped"
            );
        }
        // Second acquire should succeed (lock was released).
        let lock2 = SessionLock::acquire(&id, Duration::from_secs(1), "test2").unwrap();
        drop(lock2);
        cleanup(&id);
    }

    #[test]
    fn second_acquire_times_out_while_first_held() {
        let id = unique_id();
        let lock = SessionLock::acquire(&id, Duration::from_secs(1), "first").unwrap();
        let result = SessionLock::acquire(&id, Duration::from_millis(300), "second");
        assert!(
            result.is_err(),
            "second acquire should time out while first is held"
        );
        let err = result.err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("timed out"),
            "expected timeout error, got: {msg}"
        );
        // Attribution: the error mentions the first holder's command.
        assert!(
            msg.contains("\"first\""),
            "error should mention holder command 'first', got: {msg}"
        );
        drop(lock);
        cleanup(&id);
    }

    #[test]
    fn second_acquire_succeeds_after_first_releases() {
        let id = unique_id();
        let id_clone = id.clone();
        let first_done = Arc::new(AtomicBool::new(false));
        let first_done_clone = first_done.clone();

        let handle = thread::spawn(move || {
            let lock = SessionLock::acquire(&id_clone, Duration::from_secs(2), "first").unwrap();
            thread::sleep(Duration::from_millis(200));
            first_done_clone.store(true, Ordering::SeqCst);
            drop(lock);
        });

        // Give the first thread time to acquire.
        thread::sleep(Duration::from_millis(50));
        // Now try to acquire — we should block until first releases.
        let start = Instant::now();
        let lock2 = SessionLock::acquire(&id, Duration::from_secs(2), "second").unwrap();
        let elapsed = start.elapsed();
        assert!(
            first_done.load(Ordering::SeqCst),
            "second acquire returned before first released — release ordering broken"
        );
        assert!(
            elapsed >= Duration::from_millis(100),
            "second acquire returned too quickly ({elapsed:?}) — should have waited for first"
        );
        drop(lock2);
        handle.join().unwrap();
        cleanup(&id);
    }

    #[test]
    fn inside_tx_detects_env_var() {
        // SAFETY: setting/removing env vars from a single test thread.
        // Other tests do not read TX_ENV, so this isolation is OK in
        // practice for cargo's default thread-per-test mode.
        unsafe {
            std::env::remove_var(TX_ENV);
        }
        assert!(!SessionLock::inside_tx());
        unsafe {
            std::env::set_var(TX_ENV, "1");
        }
        assert!(SessionLock::inside_tx());
        unsafe {
            std::env::remove_var(TX_ENV);
        }
        assert!(!SessionLock::inside_tx());
    }

    #[test]
    fn sidecar_records_pid_and_command() {
        let id = unique_id();
        let lock = SessionLock::acquire(&id, Duration::from_secs(1), "editor write foo.R").unwrap();
        let sidecar = lock_dir()
            .unwrap()
            .join(format!("session-{id}.holder.json"));
        let content = fs::read_to_string(&sidecar).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            v.get("pid").and_then(|x| x.as_u64()),
            Some(std::process::id() as u64)
        );
        assert_eq!(
            v.get("command").and_then(|x| x.as_str()),
            Some("editor write foo.R")
        );
        assert!(v.get("started_ms").and_then(|x| x.as_u64()).unwrap_or(0) > 0);
        drop(lock);
        cleanup(&id);
    }

    #[test]
    fn three_concurrent_acquirers_serialize() {
        let id = unique_id();
        let order = Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));
        let mut handles = vec![];
        for i in 0..3u32 {
            let id_c = id.clone();
            let order_c = order.clone();
            handles.push(thread::spawn(move || {
                let lock =
                    SessionLock::acquire(&id_c, Duration::from_secs(5), &format!("worker-{i}"))
                        .unwrap();
                order_c.lock().unwrap().push(i);
                thread::sleep(Duration::from_millis(50));
                drop(lock);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let final_order = order.lock().unwrap().clone();
        assert_eq!(
            final_order.len(),
            3,
            "all three should have acquired exactly once"
        );
        cleanup(&id);
    }
}
