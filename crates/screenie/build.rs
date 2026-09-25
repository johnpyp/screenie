//! Stamps the binary with the git commit and commit time it was built from, as
//! `SCREENIE_COMMIT` (e.g. `46bce20 2026-09-25 23:20`). Shown by `screenie --version` and
//! `screenie status`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let commit = git(&["log", "-1", "--format=%h %cd", "--date=format:%Y-%m-%d %H:%M"])
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown commit".into());
    println!("cargo:rustc-env=SCREENIE_COMMIT={commit}");

    // Re-stamp when HEAD moves. Missing paths are skipped: cargo would treat them as
    // always changed and rebuild every time.
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) else { return };
    let git_dir = Path::new(&git_dir);
    let mut watch = vec![git_dir.join("HEAD"), git_dir.join("packed-refs")];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(git_dir.join(branch));
    }
    for path in watch.into_iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
