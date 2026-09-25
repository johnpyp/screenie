//! Stamps the binary with the git commit and commit time it was built from, as
//! `SCREENIE_COMMIT` (e.g. `46bce20 2026-09-25 23:20`, plus ` dirty` for uncommitted
//! changes). Shown by `screenie --version` and `screenie status`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let commit = git(&["log", "-1", "--format=%h %cd", "--date=format:%Y-%m-%d %H:%M"]).filter(|c| !c.is_empty());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let stamp = match commit {
        Some(c) if dirty => format!("{c} dirty"),
        Some(c) => c,
        None => "unknown commit".into(),
    };
    println!("cargo:rustc-env=SCREENIE_COMMIT={stamp}");

    // Re-stamp when HEAD moves, the index changes, or any source changes (for the dirty
    // flag). Only this leaf crate is rebuilt, so this stays cheap. Missing paths are
    // skipped: cargo would treat them as always changed.
    let mut watch = Vec::new();
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let git_dir = Path::new(&git_dir);
        watch.extend(["HEAD", "index", "packed-refs"].map(|f| git_dir.join(f)));
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch.push(git_dir.join(branch));
        }
    }
    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        watch.push(Path::new(&dir).join("../../crates"));
    }
    for path in watch.into_iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
