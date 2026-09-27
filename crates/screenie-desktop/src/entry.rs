//! Screenie's desktop entry, `dev.johnpyp.Screenie.desktop`, and its icon.
//!
//! Desktops know apps by their entries. GNOME and KDE put an app's name and icon on its
//! windows and notifications from it; the portals only take an app id that has one; and
//! KWin (up to Plasma 6.7) lets a process use its screenshots and screen casts only when
//! the entry whose `Exec` is that process's binary asks for them. So the entry must
//! name the binary that's running, which moves with every upgrade through mise or a
//! tarball: [`ensure`] rewrites it whenever it's out of date. A packaged install whose
//! system-wide entry already names this binary needs none of its own.

use std::path::{Path, PathBuf};

use screenie_config::Paths;
use screenie_core::APP_ID;

const ICON: &str = include_str!("../../../assets/icons/dev.johnpyp.Screenie.svg");

/// What [`ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The entry was up to date.
    Current,
    /// It was written or rewritten: the desktop may need telling (see [`ensure`]).
    Written,
}

/// A command the entry offers, with the key the desktop binds to it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Its id in the entry, e.g. `record`.
    pub id: String,
    pub name: String,
    /// Arguments to `screenie`.
    pub args: Vec<String>,
    /// Keys for KDE's global shortcuts (`X-KDE-Shortcuts`), e.g. `Meta+Shift+S,Print`.
    pub kde_keys: Option<String>,
}

impl Action {
    fn new(id: &str, name: &str, args: &[&str]) -> Action {
        Action {
            id: id.into(),
            name: name.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            kde_keys: None,
        }
    }
}

/// The actions every entry has; shortcuts give some of them keys.
pub fn default_actions() -> Vec<Action> {
    vec![
        Action::new("screen", "Screenshot of the Screen", &["shot", "screen"]),
        Action::new("record", "Record the Screen", &["record"]),
    ]
}

/// The entry's path in `$XDG_DATA_HOME`.
pub fn path() -> PathBuf {
    Paths::get()
        .data_home()
        .join("applications")
        .join(format!("{APP_ID}.desktop"))
}

fn icon_path() -> PathBuf {
    Paths::get()
        .data_home()
        .join("icons/hicolor/scalable/apps")
        .join(format!("{APP_ID}.svg"))
}

/// Make sure the desktop knows screenie as the binary that's running, with `actions`
/// (and their KDE keys). Tells KDE's app database of a change itself.
pub fn ensure(actions: &[Action]) -> std::io::Result<Outcome> {
    let exe = std::env::current_exe()?.canonicalize()?;
    write_if_changed(&icon_path(), ICON)?;
    let path = path();
    // A packaged entry for this very binary does the job, unless there are keys to add.
    if actions.iter().all(|a| a.kde_keys.is_none()) && system_entry_for(&exe) {
        if path.exists() {
            std::fs::remove_file(&path)?;
            refresh_kde();
            return Ok(Outcome::Written);
        }
        return Ok(Outcome::Current);
    }
    let outcome = write_if_changed(&path, &render(&exe, actions))?;
    if outcome == Outcome::Written {
        tracing::info!(path = %path.display(), "installed the desktop entry");
        refresh_kde();
    }
    Ok(outcome)
}

/// The entry for `exe`.
pub fn render(exe: &Path, actions: &[Action]) -> String {
    let command = |args: &[String]| {
        std::iter::once(quote(&exe.to_string_lossy()))
            .chain(args.iter().map(|a| quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let ids: String = actions.iter().map(|a| format!("{};", a.id)).collect();
    let mut entry = format!(
        "[Desktop Entry]
Type=Application
Name=Screenie
GenericName=Screenshot Tool
Comment=Screenshots and screen recordings
Exec={exec}
Icon={APP_ID}
Terminal=false
StartupNotify=false
Categories=Graphics;Utility;
Keywords=screenshot;screen;capture;record;recording;annotate;
X-GNOME-UsesNotifications=true
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
X-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1,org_kde_plasma_window_management
Actions={ids}
",
        exec = command(&["shot".into()]),
    );
    for action in actions {
        entry.push_str(&format!(
            "\n[Desktop Action {}]\nName={}\nExec={}\n",
            action.id,
            action.name,
            command(&action.args)
        ));
        if let Some(keys) = &action.kde_keys {
            entry.push_str(&format!("X-KDE-Shortcuts={keys}\n"));
        }
    }
    entry
}

/// An argument quoted as desktop entries need it.
fn quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@".contains(c));
    if plain {
        return arg.to_string();
    }
    let escaped: String = arg
        .chars()
        .flat_map(|c| match c {
            '"' | '`' | '$' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    // `%` is a field code; a literal one is doubled. And `\` itself is escaped again
    // by the entry's own string escaping.
    format!("\"{}\"", escaped.replace('%', "%%").replace('\\', "\\\\"))
}

/// Whether a system-wide entry (a package's) already runs `exe`.
fn system_entry_for(exe: &Path) -> bool {
    let dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.split(':').filter(|d| !d.is_empty()).any(|dir| {
        let entry = Path::new(dir)
            .join("applications")
            .join(format!("{APP_ID}.desktop"));
        std::fs::read_to_string(entry)
            .ok()
            .and_then(|text| exec_binary(&text))
            .and_then(|bin| resolve(&bin))
            .is_some_and(|bin| bin == exe)
    })
}

/// The binary an entry's main `Exec` runs.
fn exec_binary(text: &str) -> Option<String> {
    let exec = text
        .lines()
        .skip_while(|l| l.trim() != "[Desktop Entry]")
        .take_while(|l| !l.starts_with("[Desktop Action"))
        .find_map(|l| l.strip_prefix("Exec="))?;
    let exec = exec.trim();
    Some(match exec.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?.to_string(),
        None => exec.split_whitespace().next()?.to_string(),
    })
}

/// `bin` as a canonical path, looked up in `$PATH` if it's a bare name.
fn resolve(bin: &str) -> Option<PathBuf> {
    let path = if bin.contains('/') {
        PathBuf::from(bin)
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(bin))
            .find(|p| p.is_file())?
    };
    path.canonicalize().ok()
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<Outcome> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == contents) {
        return Ok(Outcome::Current);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(Outcome::Written)
}

/// Have KDE rebuild its app database (KSycoca), which KWin checks entries against and
/// kglobalaccel reads shortcuts from. Elsewhere, desktops watch the directory.
fn refresh_kde() {
    if crate::Desktop::current() != crate::Desktop::Kde {
        return;
    }
    for tool in ["kbuildsycoca6", "kbuildsycoca5"] {
        match std::process::Command::new(tool)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!("{tool}: {e}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_runs_this_binary() {
        let mut actions = default_actions();
        actions[1].kde_keys = Some("Meta+Shift+R".into());
        let entry = render(Path::new("/opt/my tools/screenie"), &actions);
        assert!(entry.contains("Exec=\"/opt/my tools/screenie\" shot\n"));
        assert!(entry.contains("Actions=screen;record;\n"));
        assert!(entry.contains("[Desktop Action record]\nName=Record the Screen\nExec=\"/opt/my tools/screenie\" record\nX-KDE-Shortcuts=Meta+Shift+R\n"));
        assert_eq!(exec_binary(&entry).as_deref(), Some("/opt/my tools/screenie"));
        let plain = render(Path::new("/usr/bin/screenie"), &[]);
        assert_eq!(exec_binary(&plain).as_deref(), Some("/usr/bin/screenie"));
    }

    #[test]
    fn arguments_are_quoted_where_they_need_it() {
        assert_eq!(quote("shot"), "shot");
        assert_eq!(quote("100,100 800x600"), "\"100,100 800x600\"");
        assert_eq!(quote("50%"), "\"50%%\"");
    }
}
