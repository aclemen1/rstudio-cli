//! End-to-end integration tests for `rstudio observe replay`.
//!
//! Replay does not require RStudio (reads a file, writes stdout — no
//! session detection, no RPC), so unlike `tests/locking.rs` these
//! tests run unconditionally and serve as smoke tests for any agent /
//! consumer of captured streams.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_rstudio");

fn write_fixture(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

const FIXTURE: &str = "\
{\"ts\":\"2026-05-04T10:00:00.000Z\",\"type\":\"editor.opened\",\"payload\":{\"id\":\"A\"}}
{\"ts\":\"2026-05-04T10:00:00.500Z\",\"type\":\"editor.dirty\",\"payload\":{\"id\":\"A\",\"dirty\":true}}
{\"ts\":\"2026-05-04T10:00:01.000Z\",\"type\":\"editor.saved\",\"payload\":{\"id\":\"A\"}}
";

#[test]
fn replay_forwards_lines_in_order() {
    let f = write_fixture(FIXTURE);
    let out = Command::new(BIN)
        .args([
            "observe",
            "replay",
            f.path().to_str().unwrap(),
            "--speed",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 lines, got {lines:?}");
    assert!(lines[0].contains("editor.opened"));
    assert!(lines[1].contains("editor.dirty"));
    assert!(lines[2].contains("editor.saved"));
}

/// `--speed 0` skips all delays; total wall-clock should be just
/// binary startup + parsing, not the original 1-second span.
#[test]
fn replay_speed_zero_is_instant() {
    let f = write_fixture(FIXTURE);
    let start = Instant::now();
    let out = Command::new(BIN)
        .args([
            "observe",
            "replay",
            f.path().to_str().unwrap(),
            "--speed",
            "0",
        ])
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(out.status.success());
    assert!(
        elapsed < Duration::from_millis(500),
        "expected < 500ms (instant), got {elapsed:?}"
    );
}

/// At `--speed 1`, the 1-second span between first and last line in
/// the fixture should be respected. Total elapsed ≈ 1s + binary
/// startup overhead.
#[test]
fn replay_speed_one_respects_cadence() {
    let f = write_fixture(FIXTURE);
    let start = Instant::now();
    let out = Command::new(BIN)
        .args([
            "observe",
            "replay",
            f.path().to_str().unwrap(),
            "--speed",
            "1",
        ])
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(out.status.success());
    // Fixture spans 1s; with speed 1 we expect at least 950ms (slack
    // for OS scheduling) and well under 2s.
    assert!(
        elapsed >= Duration::from_millis(950),
        "expected ≥ 950ms (1s span at speed 1), got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2000),
        "expected < 2000ms, got {elapsed:?}"
    );
}

/// At `--speed 10`, the 1-second span shrinks to ~100ms.
#[test]
fn replay_speed_ten_compresses_cadence() {
    let f = write_fixture(FIXTURE);
    let start = Instant::now();
    let out = Command::new(BIN)
        .args([
            "observe",
            "replay",
            f.path().to_str().unwrap(),
            "--speed",
            "10",
        ])
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(out.status.success());
    // 1s / 10 = 100ms target; allow up to 600ms for binary startup +
    // OS scheduling (debug builds are slow).
    assert!(
        elapsed < Duration::from_millis(600),
        "expected < 600ms (1s/10 + slack), got {elapsed:?}"
    );
}

/// Stdin input via `-` argument must work the same as a file path.
#[test]
fn replay_from_stdin() {
    let mut child = Command::new(BIN)
        .args(["observe", "replay", "-", "--speed", "0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(FIXTURE.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<_> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 3);
}

/// Lines without a `ts` field, or that are not valid JSON, are
/// forwarded as-is and don't affect the timing baseline.
#[test]
fn replay_forwards_malformed_lines() {
    let mixed = format!(
        "{}{}\n{}",
        FIXTURE, "garbage that is not json", "{\"no_ts\": true}"
    );
    let f = write_fixture(&mixed);
    let out = Command::new(BIN)
        .args([
            "observe",
            "replay",
            f.path().to_str().unwrap(),
            "--speed",
            "0",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("editor.opened"));
    assert!(stdout.contains("garbage"));
    assert!(stdout.contains("no_ts"));
}

/// Missing input file → user-error envelope, non-zero exit.
#[test]
fn replay_missing_file_errors_cleanly() {
    let out = Command::new(BIN)
        .args(["observe", "replay", "/tmp/no-such-file-here-please.jsonl"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("cannot open"),
        "expected 'cannot open' in error output, got: {combined}"
    );
}
