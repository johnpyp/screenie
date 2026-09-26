//! Output file naming.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

/// What was captured, for the `{app}` and `{title}` placeholders in file names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subject {
    /// The captured window's app id (or class).
    pub app: Option<String>,
    /// The captured window's title.
    pub title: Option<String>,
}

impl Subject {
    /// A short app name: `org.gnome.Nautilus` → `Nautilus`, `firefox` → `firefox`.
    fn app_name(&self) -> Option<&str> {
        let app = self.app.as_deref()?.trim();
        let name = if app.matches('.').count() >= 2 {
            app.rsplit('.').next()?
        } else {
            app
        };
        (!name.is_empty()).then_some(name)
    }
}

/// Characters that separate parts of a name, dropped along with an empty placeholder.
const SEPARATORS: &[char] = &['_', '-', ' ', '.'];

/// Expand `{app}` and `{title}` (empty when not capturing a window, taking one adjacent
/// separator with them), then strftime codes for `when`, making the result safe as a
/// file name. Invalid templates fall back to the literal text rather than failing a
/// capture.
pub fn expand_template(template: &str, when: DateTime<Local>, subject: &Subject) -> String {
    use std::fmt::Write;
    let mut text = template.to_string();
    for (placeholder, value) in [
        ("{app}", subject.app_name()),
        ("{title}", subject.title.as_deref()),
    ] {
        text = substitute(
            &text,
            placeholder,
            value.map(clean_part).filter(|v| !v.is_empty()).as_deref(),
        );
    }
    let mut out = String::new();
    let items = chrono::format::StrftimeItems::new(&text);
    if write!(out, "{}", when.format_with_items(items)).is_err() {
        out = text;
    }
    let cleaned: String = out
        .chars()
        .map(|c| if c == '/' || c == '\0' { '-' } else { c })
        .collect();
    if cleaned.trim().is_empty() {
        "capture".into()
    } else {
        cleaned
    }
}

fn substitute(text: &str, placeholder: &str, value: Option<&str>) -> String {
    let mut out = text.to_string();
    while let Some(at) = out.find(placeholder) {
        let end = at + placeholder.len();
        match value {
            // strftime runs afterwards, so `%` in a title must stay literal.
            Some(v) => out.replace_range(at..end, &v.replace('%', "%%")),
            None => {
                let before = out[..at]
                    .chars()
                    .next_back()
                    .filter(|c| SEPARATORS.contains(c));
                let after = out[end..].chars().next().filter(|c| SEPARATORS.contains(c));
                match (before, after) {
                    (Some(c), _) => out.replace_range(at - c.len_utf8()..end, ""),
                    (None, Some(c)) => out.replace_range(at..end + c.len_utf8(), ""),
                    (None, None) => out.replace_range(at..end, ""),
                }
            }
        }
    }
    out
}

/// A window's name or title as part of a file name: no path separators or control
/// characters, whitespace collapsed, and not too long.
fn clean_part(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cleaned
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

/// `dir/stem.ext`, then `dir/stem-2.ext`, `dir/stem-3.ext`, …
fn candidates<'a>(
    dir: &'a Path,
    stem: &'a str,
    ext: &'a str,
) -> impl Iterator<Item = PathBuf> + 'a {
    std::iter::once(dir.join(format!("{stem}.{ext}")))
        .chain((2..).map(move |n| dir.join(format!("{stem}-{n}.{ext}"))))
}

/// The first of `dir/stem.ext`, `dir/stem-2.ext`, `dir/stem-3.ext`, … that's free now.
/// Someone else may take it before it's written; [`claim_unique`] makes sure they can't.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    candidates(dir, stem, ext)
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// Like [`unique_path`], but the name is taken on the spot, as an empty file (and `dir`
/// created), so captures named in the same second can't pick the same one. Write the
/// file by replacing it (a rename), or remove it if nothing comes of it.
pub fn claim_unique(dir: &Path, stem: &str, ext: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for path in candidates(dir, stem, ext) {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn expands_and_sanitizes() {
        let t = Local.with_ymd_and_hms(2026, 9, 25, 21, 5, 9).unwrap();
        let none = Subject::default();
        assert_eq!(
            expand_template("Shot %Y-%m-%d %H.%M.%S", t, &none),
            "Shot 2026-09-25 21.05.09"
        );
        assert_eq!(expand_template("a/b %Y", t, &none), "a-b 2026");
        assert_eq!(expand_template("bad %Q", t, &none), "bad %Q");
    }

    #[test]
    fn window_placeholders() {
        let t = Local.with_ymd_and_hms(2026, 9, 25, 21, 5, 9).unwrap();
        let win = |app: &str, title: &str| Subject {
            app: Some(app.into()),
            title: Some(title.into()),
        };
        let template = "Screenshot_%Y-%m-%d_{app}";
        assert_eq!(
            expand_template(template, t, &win("firefox", "x")),
            "Screenshot_2026-09-25_firefox"
        );
        assert_eq!(
            expand_template(template, t, &win("org.gnome.Nautilus", "x")),
            "Screenshot_2026-09-25_Nautilus"
        );
        // No window: the placeholder and its separator disappear.
        assert_eq!(
            expand_template(template, t, &Subject::default()),
            "Screenshot_2026-09-25"
        );
        assert_eq!(expand_template("{app}-%Y", t, &Subject::default()), "2026");
        // Titles are made safe, and their % signs stay literal.
        assert_eq!(
            expand_template("{title}", t, &win("a", " 100% done / ok\n")),
            "100% done - ok"
        );
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

    #[test]
    fn claims_are_never_shared() {
        let dir = std::env::temp_dir()
            .join(format!("screenie-claim-{}", std::process::id()))
            .join("new");
        let claims: Vec<PathBuf> = std::thread::scope(|s| {
            let threads: Vec<_> = (0..8)
                .map(|_| s.spawn(|| claim_unique(&dir, "x", "png").unwrap()))
                .collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect()
        });
        let mut names: Vec<_> = claims.iter().map(|p| p.file_name().unwrap()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 8);
        assert!(claims.contains(&dir.join("x.png")) && claims.contains(&dir.join("x-8.png")));
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
