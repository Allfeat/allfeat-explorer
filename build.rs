//! Stamps `GIT_SHA` into the compiled binary via `cargo:rustc-env`.
//!
//! The server's `/api/v1/meta` endpoint reads it back with `env!` so
//! operators (and the UI footer) can see *which* commit the running
//! process came from. We rerun this script whenever `HEAD` moves or a
//! new ref is written — that covers both regular commits and rebase /
//! checkout operations — so the baked-in value never goes stale
//! silently.
//!
//! Dockerfiles that build outside a git checkout can pre-populate
//! `GIT_SHA` in the environment and we pass it through untouched.

use std::process::Command;

fn main() {
    // Honour an externally provided sha (CI/Docker builds where `.git`
    // isn't present). Otherwise shell out to `git rev-parse`.
    let sha = std::env::var("GIT_SHA")
        .ok()
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=7", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_SHA={sha}");
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}
