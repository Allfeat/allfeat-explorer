//! Phase 7 — operational gating: the binary refuses to boot in
//! `--mode=server` / `--mode=indexer` without `DATABASE_URL`.
//!
//! Spawning the full binary is heavier than a library-level unit test,
//! but the guarantee we want to lock covers the argv parsing AND the
//! env-lookup composition at the same time — the only place that sees
//! the whole stack is `main()`. The CLI surface is small (one flag +
//! one env var), so the subprocess round-trip is worth the couple of
//! seconds.
//!
//! `env!("CARGO_BIN_EXE_<name>")` points at the binary cargo just
//! built for this test binary; cargo rebuilds it if `main.rs`
//! changed, so the assertion always runs against the current code.

#![cfg(all(feature = "ssr", not(feature = "mock")))]

use std::process::Command;
use std::time::{Duration, Instant};

/// Exit code chosen by main.rs for the "misconfigured mode" refusal.
/// Split from `1` (invalid flag) so operators scripting boot sequences
/// can tell "bad CLI" apart from "missing env".
const EXPECTED_EXIT_CODE: i32 = 2;

/// `--mode=server` without DATABASE_URL exits non-zero and writes a
/// diagnostic to stderr. We match the stderr fragment rather than the
/// exact string so a future copy tweak (wording, punctuation) doesn't
/// break the test — the actionable part is "DATABASE_URL" being
/// named as the missing piece.
#[test]
fn server_mode_refuses_without_db() {
    let exe = env!("CARGO_BIN_EXE_allfeat-explorer");
    let output = run_bounded(
        Command::new(exe)
            .arg("--mode=server")
            .env_remove("DATABASE_URL"),
        Duration::from_secs(10),
    );

    assert!(
        !output.status.success(),
        "server mode should refuse without DATABASE_URL, got success exit\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(EXPECTED_EXIT_CODE),
        "expected exit code {EXPECTED_EXIT_CODE}, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DATABASE_URL"),
        "stderr must name DATABASE_URL so operators know what's missing, got:\n{stderr}"
    );
}

/// Same contract for `--mode=indexer`. The indexer *writes* to
/// Postgres, so refusing without a DB is the same shape as server
/// mode — we lock both branches so a future refactor that accidentally
/// collapses `requires_database()` back to a single variant surfaces
/// here.
#[test]
fn indexer_mode_refuses_without_db() {
    let exe = env!("CARGO_BIN_EXE_allfeat-explorer");
    let output = run_bounded(
        Command::new(exe)
            .arg("--mode=indexer")
            .env_remove("DATABASE_URL"),
        Duration::from_secs(10),
    );

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(EXPECTED_EXIT_CODE));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DATABASE_URL"),
        "stderr must name DATABASE_URL:\n{stderr}"
    );
}

/// Passing an unknown value to `--mode` is rejected with the
/// dedicated invalid-flag exit code (1). Keeping this distinct from
/// the missing-DB path prevents a typo like `--mode=indexr` from
/// looking like a "DB config" issue to operators — they'd chase the
/// wrong thread.
#[test]
fn unknown_mode_exits_with_invalid_flag_code() {
    let exe = env!("CARGO_BIN_EXE_allfeat-explorer");
    let output = run_bounded(
        Command::new(exe).arg("--mode=bogus"),
        Duration::from_secs(10),
    );

    assert!(!output.status.success());
    assert_eq!(
        output.status.code(),
        Some(1),
        "invalid --mode should use exit code 1 (invalid flag), got {:?}",
        output.status.code()
    );
}

/// Spawn `cmd` and kill it after `timeout` if it's still running.
/// Needed because main.rs only exits for the error paths — a missing
/// assertion here would let a test hang indefinitely while the
/// listener happily stays up. Returns the captured output either way.
fn run_bounded(cmd: &mut Command, timeout: Duration) -> std::process::Output {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn binary");
    let start = Instant::now();
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => break,
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    panic!("binary did not exit within {timeout:?}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    child.wait_with_output().expect("collect output")
}
