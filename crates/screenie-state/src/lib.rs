//! What screenie remembers between runs, as opposed to what the user sets
//! (`screenie-config`): the editor's last style, the last captures, and the desktop
//! shortcuts it took over.
//!
//! It lives in `$XDG_STATE_HOME/screenie/state.yaml`. [`StateFile`] is the only reader
//! and writer. The file carries a `version`; older files are brought up to date by the
//! migration chain in [`migrate`] before being read, so [`State`] only ever describes the
//! current layout. Nothing here is precious: a broken file is set aside and screenie
//! starts afresh.

mod migrate;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use screenie_config::Paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use migrate::VERSION;

/// Everything remembered. Every field is optional: absent means "nothing yet", and the
/// caller falls back to the configured default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub editor: EditorState,
    pub last: LastState,
    pub portal: PortalState,
    pub shortcuts: ShortcutsState,
}

/// The editor's style as last used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorState {
    /// `#rrggbb` or `#rrggbbaa`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Stroke width in logical pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// Filled shapes and labelled text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<bool>,
}

/// The latest captures, so `shot last` and `query last` survive a daemon restart (an
/// upgrade, a crash, logging out).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LastState {
    /// The region of the latest capture, for `screenie shot last` / `record last`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<Capture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording: Option<Capture>,
}

/// What xdg-desktop-portal handed back to use next time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortalState {
    /// Restores the screens chosen for the last screen cast without asking again. Good
    /// once: each cast replaces it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screencast_token: Option<String>,
}

/// Where screenie has the desktop's screenshot keys (`screenie shortcuts install`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutsState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gnome: Option<Shortcuts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kde: Option<Shortcuts>,
}

/// Screenie's keys on one desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shortcuts {
    /// What they run.
    pub command: PathBuf,
    /// Keys taken from the desktop's own shortcuts, to give back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taken: Vec<TakenKey>,
}

/// A key taken from one of the desktop's own shortcuts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakenKey {
    /// The shortcut: a GNOME setting (schema and key), a KDE action (component, action,
    /// and their names).
    pub from: Vec<String>,
    /// The key, as that desktop writes it.
    pub key: String,
}

/// A rectangle in the compositor's logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A finished capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// Where it was saved; absent if it was only copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// When it was taken, in Unix seconds.
    pub time: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("serializing state: {0}")]
    Serialize(#[from] serde_saphyr::SerializeError),
}

/// Why a state file couldn't be used.
#[derive(Debug, thiserror::Error)]
enum LoadError {
    #[error("{0}")]
    Read(#[from] std::io::Error),
    #[error("not valid YAML: {0}")]
    Parse(#[from] Box<serde_saphyr::Error>),
    #[error("no version number")]
    NoVersion,
    #[error("written by a newer screenie (version {0}, this one knows up to {VERSION})")]
    Newer(u64),
    #[error("doesn't match version {VERSION}: {0}")]
    Shape(#[from] serde_json::Error),
}

const HEADER: &str = "\
# What screenie remembers between runs (settings are in config.yaml). Rewritten by
# screenie; safe to delete.

";

/// The state file: loaded once, then kept in step with every [`StateFile::update`]. Safe
/// to share between threads.
#[derive(Debug)]
pub struct StateFile {
    path: PathBuf,
    state: Mutex<State>,
    /// False for a file from a newer screenie, which we mustn't overwrite with less.
    writable: bool,
}

impl StateFile {
    /// Open `$XDG_STATE_HOME/screenie/state.yaml`.
    pub fn open() -> Self {
        Self::open_at(Paths::get().state_file())
    }

    /// Open the state file at `path`. Never fails: a missing file is empty state, and a
    /// broken one is renamed to `*.bad` (with a warning) and replaced.
    pub fn open_at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (state, writable) = match load(&path) {
            Ok(state) => (state, true),
            Err(LoadError::Read(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                (State::default(), true)
            }
            Err(e @ LoadError::Newer(_)) => {
                tracing::warn!("{}: {e}; not remembering anything this run", path.display());
                (State::default(), false)
            }
            Err(e) => {
                let aside = path.with_extension("yaml.bad");
                tracing::warn!(
                    "{}: {e}; moved it to {} and starting afresh",
                    path.display(),
                    aside.display()
                );
                let _ = std::fs::rename(&path, &aside);
                (State::default(), true)
            }
        };
        Self {
            path,
            state: Mutex::new(state),
            writable,
        }
    }

    /// What's remembered now.
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Change the state, writing the file if anything changed.
    pub fn update(&self, change: impl FnOnce(&mut State)) -> Result<(), Error> {
        let mut current = self.state.lock().unwrap();
        let mut state = current.clone();
        change(&mut state);
        if state == *current {
            return Ok(());
        }
        *current = state;
        if self.writable {
            self.save(&current)
        } else {
            Ok(())
        }
    }

    /// Write atomically, so a reader never sees half a file.
    fn save(&self, state: &State) -> Result<(), Error> {
        #[derive(Serialize)]
        struct OnDisk<'a> {
            version: u32,
            #[serde(flatten)]
            state: &'a State,
        }
        let text = format!(
            "{HEADER}{}",
            serde_saphyr::to_string(&OnDisk {
                version: VERSION,
                state
            })?
        );
        let werr = |source| Error::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(werr)?;
        }
        let tmp = self.path.with_extension("yaml.tmp");
        std::fs::write(&tmp, text).map_err(werr)?;
        std::fs::rename(&tmp, &self.path).map_err(werr)
    }
}

fn load(path: &Path) -> Result<State, LoadError> {
    parse(&std::fs::read_to_string(path)?)
}

/// Read a state file of any known version as the current [`State`].
fn parse(text: &str) -> Result<State, LoadError> {
    let mut doc: Value = serde_saphyr::from_str(text).map_err(Box::new)?;
    let version = doc
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(LoadError::NoVersion)?;
    if version > u64::from(VERSION) {
        return Err(LoadError::Newer(version));
    }
    if let Some(map) = doc.as_object_mut() {
        map.remove("version");
    }
    Ok(serde_json::from_value(migrate::migrate(
        doc,
        version as u32,
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test, removed when it's dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("screenie-state-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn state_file(&self) -> PathBuf {
            self.0.join("state.yaml")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn remembered() -> EditorState {
        EditorState {
            color: Some("#0a84ff".into()),
            size: Some(8.0),
            fill: Some(true),
        }
    }

    #[test]
    fn missing_file_is_empty_and_updates_are_written_back() {
        let scratch = Scratch::new("roundtrip");
        let path = scratch.state_file();
        let file = StateFile::open_at(&path);
        assert_eq!(file.state(), State::default());
        file.update(|s| s.editor = remembered()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("version: {VERSION}")), "{text}");
        assert_eq!(StateFile::open_at(&path).state().editor, remembered());
    }

    #[test]
    fn broken_file_is_set_aside() {
        let scratch = Scratch::new("broken");
        let path = scratch.state_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "editor: [not, a, map").unwrap();
        let file = StateFile::open_at(&path);
        assert_eq!(file.state(), State::default());
        assert!(path.with_extension("yaml.bad").exists());
        assert!(!path.exists());
    }

    #[test]
    fn unversioned_file_is_set_aside() {
        let scratch = Scratch::new("unversioned");
        let path = scratch.state_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "editor: { size: 8 }\n").unwrap();
        assert_eq!(StateFile::open_at(&path).state(), State::default());
        assert!(path.with_extension("yaml.bad").exists());
    }

    #[test]
    fn newer_file_is_left_alone() {
        let scratch = Scratch::new("newer");
        let path = scratch.state_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let newer = format!("version: {}\neditor: {{ size: 8 }}\n", VERSION + 1);
        std::fs::write(&path, &newer).unwrap();
        let file = StateFile::open_at(&path);
        assert_eq!(file.state(), State::default());
        file.update(|s| s.editor = remembered()).unwrap();
        assert_eq!(file.state().editor, remembered(), "remembered for this run");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            newer,
            "but never written"
        );
    }

    /// Every released layout still loads, and migrating loses nothing: what a fixture
    /// migrates to reads back as the same document.
    #[test]
    fn every_version_migrates_without_loss() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for version in 1..=VERSION {
            let path = dir.join(format!("v{version}.yaml"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let mut doc: Value = serde_saphyr::from_str(&text).unwrap();
            doc.as_object_mut().unwrap().remove("version");
            let migrated = migrate::migrate(doc, version);
            let state: State = serde_json::from_value(migrated.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&state).unwrap(),
                migrated,
                "v{version} lost something on the way"
            );
            assert_eq!(parse(&text).unwrap(), state);
        }
    }

    #[test]
    fn current_fixture_is_what_we_write() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/v{VERSION}.yaml"));
        let fixture = parse(&std::fs::read_to_string(fixture).unwrap()).unwrap();
        let scratch = Scratch::new("fixture");
        let path = scratch.state_file();
        StateFile::open_at(&path)
            .update(|s| *s = fixture.clone())
            .unwrap();
        assert_eq!(StateFile::open_at(&path).state(), fixture);
    }

    #[test]
    fn unchanged_state_isnt_written() {
        let scratch = Scratch::new("unchanged");
        let path = scratch.state_file();
        let file = StateFile::open_at(&path);
        file.update(|_| {}).unwrap();
        assert!(!path.exists());
    }
}
