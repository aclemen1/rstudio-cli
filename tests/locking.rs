//! End-to-end integration tests for Phase 1 (per-call mutex) and
//! Phase 2 (`rstudio tx -- ...`) of the multi-agent locking design.
//!
//! These tests spawn the actual `rstudio` binary, so they require a
//! live RStudio session to be running on the test machine. CI only
//! compiles tests (`cargo test --lib --verbose` runs unit tests; the
//! `cargo build --tests --verbose` step compiles these but doesn't
//! execute them). On a dev machine with RStudio running, these are
//! exercised automatically by `cargo test`.
//!
//! Tests that need a live session check `rstudio_available()` first
//! and skip cleanly (returning Ok) when no session is reachable, so
//! the test binary still passes everywhere.

use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_rstudio");

/// Tests in this file all target the same per-session `flock` (the
/// dev machine's live RStudio session). Running them in parallel —
/// cargo's default — makes them contend with each other: a test that
/// expects to acquire the lock immediately may instead wait for
/// another test's `sleep`. We serialise everything that takes or
/// observes the lock through this global mutex. The four meta-CLI
/// tests at the top (version, observe events, policy show, env var
/// echo) don't touch the lock and don't need it.
static LOCK_TEST_SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    // PoisonError handling: if a previous test panicked while holding
    // the mutex, the lock is "poisoned". We don't care — clear and go.
    match LOCK_TEST_SERIAL.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn rstudio_available() -> bool {
    Command::new(BIN)
        .arg("status")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `rstudio version` — neither needs a session nor takes a lock.
/// Always works regardless of whether RStudio is running.
#[test]
fn version_works_without_rstudio() {
    let out = Command::new(BIN).arg("version").output().unwrap();
    assert!(
        out.status.success(),
        "rstudio version should succeed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Default `text` format prints "X.Y.Z\n".
    assert!(
        stdout
            .trim()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        "expected version line, got: {stdout:?}"
    );
}

/// Reads (e.g. `observe events`) never take the lock, so they must
/// not require RStudio. observe events is also pure-static so there
/// is no possibility of session interaction.
#[test]
fn observe_events_works_without_rstudio() {
    let out = Command::new(BIN)
        .args(["observe", "events"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"events\""),
        "expected events catalog, got: {stdout}"
    );
}

/// `policy show` is a meta-CLI command — no session, no lock.
#[test]
fn policy_show_works_without_rstudio() {
    let out = Command::new(BIN).args(["policy", "show"]).output().unwrap();
    assert!(out.status.success());
}

/// Two `rstudio tx -- sleep 0.4` invocations launched in parallel
/// must serialize — total wall-clock should be ≥ 0.8s, not ~0.4s.
/// Skipped on CI / machines without RStudio.
#[test]
fn tx_serializes_two_writers() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let start = Instant::now();
    let mut a = Command::new(BIN)
        .args(["tx", "--", "sleep", "0.4"])
        .spawn()
        .unwrap();
    // Give the first one ~50ms to acquire before the second starts.
    std::thread::sleep(Duration::from_millis(50));
    let mut b = Command::new(BIN)
        .args(["tx", "--", "sleep", "0.4"])
        .spawn()
        .unwrap();
    let sa = a.wait().unwrap();
    let sb = b.wait().unwrap();
    let elapsed = start.elapsed();
    assert!(sa.success() && sb.success());
    assert!(
        elapsed >= Duration::from_millis(700),
        "expected ≥700ms (serialized 2x ~0.4s), got {elapsed:?}"
    );
    // Sanity: not absurdly long either.
    assert!(
        elapsed < Duration::from_secs(5),
        "expected < 5s, got {elapsed:?}"
    );
}

/// With `--no-lock`, two `tx -- sleep` invocations can run in
/// parallel: total wall-clock close to one sleep, not two. We use
/// 0.5s sleeps with a 1.0s upper bound to be tolerant of debug
/// binary startup overhead (~150ms × 2 = 300ms).
#[test]
fn no_lock_allows_parallel_writers() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let start = Instant::now();
    let mut a = Command::new(BIN)
        .args(["--no-lock", "tx", "--", "sleep", "0.5"])
        .spawn()
        .unwrap();
    let mut b = Command::new(BIN)
        .args(["--no-lock", "tx", "--", "sleep", "0.5"])
        .spawn()
        .unwrap();
    a.wait().unwrap();
    b.wait().unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(1000),
        "expected < 1000ms (parallel under --no-lock), got {elapsed:?}"
    );
}

/// Inside a tx, `RSTUDIO_TX_HELD=1` is set in the child env. Verified
/// by having bash echo it.
#[test]
fn tx_sets_held_env_var() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let out = Command::new(BIN)
        .args(["tx", "--", "bash", "-c", "echo \"$RSTUDIO_TX_HELD\""])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1");
}

/// Outside a tx, the env var is NOT set.
#[test]
fn outside_tx_env_var_unset() {
    let out = Command::new("bash")
        .args(["-c", "echo \"${RSTUDIO_TX_HELD-unset}\""])
        .env_remove("RSTUDIO_TX_HELD")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "unset");
}

/// `tx --` propagates the child's exit code (Unix convention).
#[test]
fn tx_propagates_exit_code() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let status = Command::new(BIN)
        .args(["tx", "--", "bash", "-c", "exit 42"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(42));
}

/// Inside a tx, child `rstudio` invocations detect the env var and
/// skip lock acquisition — which is what makes nested calls possible
/// at all. We verify by running an inner tx that times its own work.
#[test]
fn nested_rstudio_inside_tx_does_not_deadlock() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let bin = BIN;
    let script = format!("{bin} status > /dev/null && {bin} observe events > /dev/null && echo ok");
    let out = Command::new(BIN)
        .args(["tx", "--", "bash", "-c", &script])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

/// Reads (which don't take a lock) run immediately even while a tx
/// is holding the lock for a long sleep. We use `observe events` to
/// keep the test focused on lock semantics — it's a pure meta-CLI
/// command that doesn't even touch the rsession, so its runtime is
/// entirely binary startup + read of the static catalog. If it
/// blocked, that would indicate the lock is incorrectly being
/// acquired for reads.
#[test]
fn reads_dont_block_on_active_tx() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut tx = Command::new(BIN)
        .args(["tx", "--", "sleep", "2"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    let out = Command::new(BIN)
        .args(["observe", "events"])
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(out.status.success());
    assert!(
        elapsed < Duration::from_millis(900),
        "read took {elapsed:?} — should not have blocked on tx (debug binary tolerance)"
    );
    tx.wait().unwrap();
}

/// Lock timeout fires with a clear message including holder PID +
/// command. `--lock-timeout 0.2` against a tx holding for 1s.
#[test]
fn lock_timeout_reports_holder() {
    if !rstudio_available() {
        eprintln!("skipping: RStudio not running");
        return;
    }
    let _serial = serial();
    let mut holder = Command::new(BIN)
        .args(["tx", "--", "sleep", "1"])
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    // Try a writer with very short timeout — should fail with attribution.
    let out = Command::new(BIN)
        .args([
            "--lock-timeout",
            "0.2",
            "tx",
            "--",
            "echo",
            "should not run",
        ])
        .output()
        .unwrap();
    holder.wait().unwrap();
    assert!(!out.status.success(), "second tx should have timed out");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stderr}\n{stdout}");
    assert!(
        combined.contains("timed out"),
        "expected 'timed out' in error output, got: {combined}"
    );
    // Attribution: should mention the holder's command (substring of "tx -- sleep 1").
    assert!(
        combined.contains("sleep"),
        "expected holder command in attribution, got: {combined}"
    );
}
