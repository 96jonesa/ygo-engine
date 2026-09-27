//! This build against ocgcore, on random games — the mutation sweep's
//! second opinion.
//!
//! Off by default. With `PORT_ORACLE_FUZZ=<games>` set it plays that many
//! random pool-deck games of **this build's** `trace` example against
//! ocgcore through `tools/fuzz.py`, and fails on any divergence. Under
//! `cargo mutants` every mutant is such a build, so a mutant that changes
//! behaviour the unit tests never look at is still caught if a random
//! game reaches it; what survives both is either equivalent or in a
//! branch the pool never plays. Games are capped at 100k steps (about four
//! times a normal one), so a mutant that keeps a game from ending is caught
//! at the cap instead of timing the whole test out. `PORT_ORACLE_REPO` names the repository
//! that has the oracle build (the mutant lives in a copy that has
//! neither); it defaults to this crate's own repository.
//!
//! The `trace` binary is the one cargo built beside this test — the
//! mutant's, not the tree's. The test (re)builds it itself before running,
//! because `cargo test --test oracle_fuzz` builds only the named target and
//! would otherwise run whatever example binary was left from an earlier
//! build.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn this_build_agrees_with_ocgcore_on_random_games() {
    let Some(games) = std::env::var_os("PORT_ORACLE_FUZZ") else {
        return;
    };
    let repo = std::env::var_os("PORT_ORACLE_REPO").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .canonicalize()
                .expect("the repository root")
        },
        PathBuf::from,
    );
    // target/<profile>/deps/oracle_fuzz-<hash> → target/<profile>/examples/trace
    let exe = std::env::current_exe().expect("this test's binary");
    let profile = exe
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap_or_else(|| panic!("no profile directory above {}", exe.display()));
    let mut build = Command::new(env!("CARGO"));
    build
        .arg("build")
        .arg("--quiet")
        .arg("--example")
        .arg("trace")
        .arg("--manifest-path")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
    if let Some(target_dir) = profile.parent() {
        build.arg("--target-dir").arg(target_dir);
    }
    if profile.file_name().is_some_and(|n| n == "release") {
        build.arg("--release");
    }
    let built = build.status().expect("cargo build --example trace runs");
    assert!(built.success(), "the trace example builds: {built}");
    let trace = profile.join("examples").join("trace");
    assert!(trace.is_file(), "no trace example at {}", trace.display());
    // The harness needs only the standard library; `PYTHON` names another
    // interpreter than `python3`.
    let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
    let status = Command::new(python)
        .arg(repo.join("tools/fuzz.py"))
        .args([
            "--deck",
            "pool",
            "--seed",
            "424242",
            "--workers",
            "2",
            // The step bound turns a mutant that stops games from ending into a
            // catch in seconds rather than a twenty-second timeout: the port
            // stops at the cap while ocgcore plays on, and the traces part. A
            // normal game is ~27k steps, so no honest game is cut short.
            "--max-steps",
            "100000",
            "--games",
        ])
        .arg(&games)
        .env("PORT_BIN", &trace)
        .status()
        .expect("fuzz.py runs");
    assert!(
        status.success(),
        "this build diverged from ocgcore (fuzz.py exit {status}); rerun \
         `tools/fuzz.py --deck pool --seed 424242 --games {}` with PORT_BIN={}",
        games.to_string_lossy(),
        trace.display()
    );
}
