//! Output file naming.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

/// Expand a strftime `template` for `when`, making the result safe as a file name.
/// Invalid templates fall back to the literal text rather than failing a capture.
pub fn expand_template(template: &str, when: DateTime<Local>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let items = chrono::format::StrftimeItems::new(template);
    if write!(out, "{}", when.format_with_items(items)).is_err() {
        out = template.to_string();
    }
    let cleaned: String = out.chars().map(|c| if c == '/' || c == '\0' { '-' } else { c }).collect();
    if cleaned.trim().is_empty() { "capture".into() } else { cleaned }
}

/// `dir/stem.ext`, or `dir/stem-2.ext`, `dir/stem-3.ext`, … if taken.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let candidate = dir.join(format!("{stem}.{ext}"));
    if !candidate.exists() {
        return candidate;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn expands_and_sanitizes() {
        let t = Local.with_ymd_and_hms(2026, 9, 25, 21, 5, 9).unwrap();
        assert_eq!(expand_template("Shot %Y-%m-%d %H.%M.%S", t), "Shot 2026-09-25 21.05.09");
        assert_eq!(expand_template("a/b %Y", t), "a-b 2026");
        assert_eq!(expand_template("bad %Q", t), "bad %Q");
    }

    #[test]
    fn unique_suffix() {
        let dir = std::env::temp_dir().join(format!("screenie-naming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "x", "png"), dir.join("x.png"));
        std::fs::write(dir.join("x.png"), b"").unwrap();
        assert_eq!(unique_path(&dir, "x", "png"), dir.join("x-2.png"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
