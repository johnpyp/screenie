//! Screenie's GNOME Shell extension (`extension/`): what GNOME gives only to code running
//! in its shell. Mutter tells no client where windows are, has no layer-shell to float
//! previews and controls over other windows, and puts every screen cast in the top bar;
//! the extension does these for screenie, over D-Bus ([`crate::shell`]).
//!
//! GNOME loads extensions when the user logs in, so an extension installed (or updated)
//! mid-session runs from the next login. Until then, screenie does without.

use std::path::{Path, PathBuf};

use screenie_config::Paths;
use zbus::zvariant::OwnedValue;

use crate::entry::write_if_changed;
use crate::gsettings;

pub const UUID: &str = "screenie@johnpyp.dev";

/// The extension's files: (name, contents).
const FILES: &[(&str, &str)] = &[
    ("metadata.json", include_str!("../extension/metadata.json")),
    ("extension.js", include_str!("../extension/extension.js")),
    ("callers.js", include_str!("../extension/callers.js")),
    ("casts.js", include_str!("../extension/casts.js")),
    ("layers.js", include_str!("../extension/layers.js")),
    ("windows.js", include_str!("../extension/windows.js")),
];

/// The GNOME Shell versions it's made for, oldest first (`metadata.json`'s).
pub const SHELL_VERSIONS: &[&str] = &["46", "47", "48", "49", "50"];

const SHELL: &str = "org.gnome.shell";

/// How the extension stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Running in the shell.
    Running,
    /// Running, but an older copy than the one just installed, which runs from the next
    /// login.
    Updated,
    /// Installed and enabled, to run from the next login.
    NextLogin,
    /// Installed, but turned off (in GNOME's Extensions app, say).
    Disabled,
    /// GNOME has extensions turned off altogether.
    ExtensionsOff,
    /// This GNOME Shell (its version) is older than any it's made for.
    TooOld(String),
    /// Or newer.
    TooNew(String),
    /// GNOME Shell couldn't run it: why.
    Failed(String),
    NotInstalled,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Settings(#[from] gsettings::Error),
    #[error("writing the extension: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The user's copy, in `$XDG_DATA_HOME`.
pub fn dir() -> PathBuf {
    Paths::get()
        .data_home()
        .join("gnome-shell/extensions")
        .join(UUID)
}

/// Install the extension for this user (unless a package has this very one installed
/// system-wide) and enable it, if it's made for this GNOME Shell.
pub fn install() -> Result<State> {
    if let Some(state) = shell().and_then(|s| unsupported(&s.version)) {
        return Ok(state);
    }
    let changed = if system_copy().is_some() {
        remove_dir(&dir())?;
        false
    } else {
        write(&dir())?
    };
    let mut enabled = gsettings::get_list(SHELL, "enabled-extensions")?;
    if !enabled.iter().any(|u| u == UUID) {
        enabled.push(UUID.into());
        gsettings::set(
            SHELL,
            "enabled-extensions",
            &gsettings::Value::List(enabled),
        )?;
    }
    let mut disabled = gsettings::get_list(SHELL, "disabled-extensions")?;
    if disabled.iter().any(|u| u == UUID) {
        disabled.retain(|u| u != UUID);
        gsettings::set(
            SHELL,
            "disabled-extensions",
            &gsettings::Value::List(disabled),
        )?;
    }
    // Enabling one the shell already knows starts it.
    Ok(match state() {
        State::Running if changed => State::Updated,
        state => state,
    })
}

/// Disable the extension, and remove the user's copy. The shell stops it at once.
pub fn remove() -> Result<()> {
    let mut enabled = gsettings::get_list(SHELL, "enabled-extensions")?;
    if enabled.iter().any(|u| u == UUID) {
        enabled.retain(|u| u != UUID);
        gsettings::set(
            SHELL,
            "enabled-extensions",
            &gsettings::Value::List(enabled),
        )?;
    }
    remove_dir(&dir())?;
    Ok(())
}

/// Bring an installed user copy up to this build's (it runs from the next login).
pub fn keep_current() {
    let dir = dir();
    if !dir.join("metadata.json").exists() {
        return;
    }
    match write(&dir) {
        Ok(true) => tracing::info!(
            dir = %dir.display(),
            "updated the GNOME Shell extension; it runs from the next login"
        ),
        Ok(false) => {}
        Err(e) => tracing::warn!("updating the GNOME Shell extension: {e}"),
    }
}

/// Write the extension into a package's `share` directory, as
/// `DIR/gnome-shell/extensions/<uuid>`.
pub fn write_packaged(share: &Path) -> std::io::Result<()> {
    write(&share.join("gnome-shell/extensions").join(UUID)).map(drop)
}

/// How the extension stands now.
pub fn state() -> State {
    let shell = shell();
    if let Some(state) = shell.as_ref().and_then(|s| unsupported(&s.version)) {
        return state;
    }
    let installed = dir().join("metadata.json").exists() || system_copy().is_some();
    let listed = |key| gsettings::get_list(SHELL, key).unwrap_or_default();
    let enabled = listed("enabled-extensions").iter().any(|u| u == UUID)
        && !listed("disabled-extensions").iter().any(|u| u == UUID);
    let extensions_off = gsettings::run(&["get", SHELL, "disable-user-extensions"])
        .is_ok_and(|v| v.trim() == "true");
    if !installed {
        return State::NotInstalled;
    }
    if extensions_off {
        return State::ExtensionsOff;
    }
    if !enabled {
        return State::Disabled;
    }
    // What the shell says of it; nothing if it was installed since the shell started. Out
    // of date (4), it's an older screenie's copy: this one's runs from the next login.
    match shell.and_then(|s| s.extension) {
        Some(info) => match info.state {
            // ACTIVE, ACTIVATING
            1 | 8 => State::Running,
            3 => State::Failed(info.error),
            _ => State::NextLogin,
        },
        None => State::NextLogin,
    }
}

/// Whether GNOME Shell `version` (like `49.1` or `50.alpha`) is one the extension isn't
/// made for, and which way.
fn unsupported(version: &str) -> Option<State> {
    let major = version.split('.').next()?;
    if SHELL_VERSIONS.contains(&major) {
        return None;
    }
    let major: u32 = major.parse().ok()?;
    let first: u32 = SHELL_VERSIONS[0].parse().ok()?;
    Some(if major < first {
        State::TooOld(version.into())
    } else {
        State::TooNew(version.into())
    })
}

/// Write the extension's files into `dir`: whether any changed.
fn write(dir: &Path) -> std::io::Result<bool> {
    let mut changed = false;
    for (name, contents) in FILES {
        changed |= write_if_changed(&dir.join(name), contents)? == crate::entry::Outcome::Written;
    }
    Ok(changed)
}

fn remove_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A system-wide copy (a package's) of this very extension.
fn system_copy() -> Option<PathBuf> {
    let dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("gnome-shell/extensions").join(UUID))
        .find(|dir| {
            FILES.iter().all(|(name, contents)| {
                std::fs::read_to_string(dir.join(name)).is_ok_and(|c| c == *contents)
            })
        })
}

/// What GNOME Shell says of itself and the extension.
struct Shell {
    version: String,
    /// Nothing if it doesn't know the extension.
    extension: Option<ShellInfo>,
}

struct ShellInfo {
    state: u32,
    error: String,
}

#[zbus::proxy(
    interface = "org.gnome.Shell.Extensions",
    default_service = "org.gnome.Shell.Extensions",
    default_path = "/org/gnome/Shell/Extensions",
    gen_blocking = false
)]
trait Extensions {
    fn get_extension_info(
        &self,
        uuid: &str,
    ) -> zbus::Result<std::collections::HashMap<String, OwnedValue>>;

    #[zbus(property)]
    fn shell_version(&self) -> zbus::Result<String>;
}

/// What GNOME Shell says, if it's running.
fn shell() -> Option<Shell> {
    let (version, info) = async_io::block_on(async {
        let connection = zbus::Connection::session().await?;
        let extensions = ExtensionsProxy::new(&connection).await?;
        zbus::Result::Ok((
            extensions.shell_version().await?,
            extensions.get_extension_info(UUID).await?,
        ))
    })
    .inspect_err(|e| tracing::debug!("GNOME Shell's extensions: {e}"))
    .ok()?;
    let extension = info
        .get("state")
        .and_then(|v| f64::try_from(v).ok())
        .map(|state| ShellInfo {
            state: state as u32,
            error: info
                .get("error")
                .and_then(|v| String::try_from(v.clone()).ok())
                .unwrap_or_default(),
        });
    Some(Shell { version, extension })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_of_the_extension_is_installed() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("extension");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut installed: Vec<String> = FILES.iter().map(|(name, _)| name.to_string()).collect();
        installed.sort();
        assert_eq!(installed, on_disk);
    }

    fn metadata() -> &'static str {
        FILES
            .iter()
            .find(|(name, _)| *name == "metadata.json")
            .unwrap()
            .1
    }

    #[test]
    fn the_uuid_is_the_metadatas() {
        assert!(metadata().contains(&format!("\"uuid\": \"{UUID}\"")));
    }

    #[test]
    fn the_shell_versions_are_the_metadatas() {
        let listed: Vec<String> = SHELL_VERSIONS.iter().map(|v| format!("\"{v}\"")).collect();
        assert!(metadata().contains(&format!("\"shell-version\": [{}]", listed.join(", "))));
    }

    #[test]
    fn shell_versions() {
        assert_eq!(unsupported("48.2"), None);
        assert_eq!(unsupported("50.alpha"), None);
        assert_eq!(unsupported("42.9"), Some(State::TooOld("42.9".into())));
        assert_eq!(
            unsupported("51.beta"),
            Some(State::TooNew("51.beta".into()))
        );
        assert_eq!(unsupported("unknown"), None);
    }
}
