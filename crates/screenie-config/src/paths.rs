//! Where screenie keeps things, per the XDG base directory spec.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::{BaseDirs, UserDirs};

pub struct Paths {
    config_dir: PathBuf,
    state_dir: PathBuf,
    runtime_dir: PathBuf,
    home: PathBuf,
    pictures: Option<PathBuf>,
    videos: Option<PathBuf>,
}

impl Paths {
    pub fn get() -> &'static Paths {
        static PATHS: OnceLock<Paths> = OnceLock::new();
        PATHS.get_or_init(|| {
            let base = BaseDirs::new();
            let user = UserDirs::new();
            let home = base
                .as_ref()
                .map(|b| b.home_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/"));
            let uid = rustix_uid();
            Paths {
                config_dir: base
                    .as_ref()
                    .map(|b| b.config_dir().to_path_buf())
                    .unwrap_or_else(|| home.join(".config"))
                    .join("screenie"),
                state_dir: base
                    .as_ref()
                    .and_then(|b| b.state_dir().map(Path::to_path_buf))
                    .unwrap_or_else(|| home.join(".local/state"))
                    .join("screenie"),
                runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::temp_dir().join(format!("screenie-{uid}")))
                    .join("screenie"),
                pictures: user
                    .as_ref()
                    .and_then(|u| u.picture_dir().map(Path::to_path_buf)),
                videos: user
                    .as_ref()
                    .and_then(|u| u.video_dir().map(Path::to_path_buf)),
                home,
            }
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Persistent state: the daemon log and what screenie remembers (`state.yaml`).
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// What screenie remembers between runs; owned by `screenie-state`.
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.yaml")
    }

    /// Per-session files: the daemon socket, temporary captures.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn socket(&self) -> PathBuf {
        // Namespaced by display so two sessions for one user get separate daemons.
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let display = display
            .rsplit('/')
            .next()
            .unwrap_or("wayland-0")
            .to_string();
        self.runtime_dir.join(format!("{display}.sock"))
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn pictures_dir(&self) -> PathBuf {
        self.pictures
            .clone()
            .unwrap_or_else(|| self.home.join("Pictures"))
    }

    pub fn videos_dir(&self) -> PathBuf {
        self.videos
            .clone()
            .unwrap_or_else(|| self.home.join("Videos"))
    }
}

/// A directory as the user wrote it in the config: `~` and `$VAR` / `${VAR}` expand, and a
/// relative path is relative to the home directory, never to wherever the daemon was
/// started from. `$XDG_PICTURES_DIR` and `$XDG_VIDEOS_DIR` work even when they're only
/// set in `user-dirs.dirs`, not in the environment.
pub fn expand_user_path(path: &Path) -> Result<PathBuf, String> {
    let paths = Paths::get();
    expand_with(path, paths.home(), |var| match std::env::var(var) {
        Ok(value) => Some(value),
        Err(_) => match var {
            "XDG_PICTURES_DIR" => Some(paths.pictures_dir().display().to_string()),
            "XDG_VIDEOS_DIR" => Some(paths.videos_dir().display().to_string()),
            _ => None,
        },
    })
}

fn expand_with(
    path: &Path,
    home: &Path,
    var: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, String> {
    // A path that isn't UTF-8 can't hold anything to expand.
    let expanded = match path.to_str() {
        Some(text) => PathBuf::from(
            shellexpand::full_with_context(
                text,
                || home.to_str(),
                |name| var(name).map(Some).ok_or(()),
            )
            .map_err(|e| format!("{}: ${} isn't set", path.display(), e.var_name))?
            .as_ref(),
        ),
        None => path.to_path_buf(),
    };
    Ok(home.join(expanded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(path: &str) -> Result<PathBuf, String> {
        expand_with(Path::new(path), Path::new("/home/me"), |var| {
            (var == "SHOTS").then(|| "/data/shots".to_string())
        })
    }

    #[test]
    fn user_paths_expand_and_are_relative_to_home() {
        assert_eq!(expand("~/Shots"), Ok("/home/me/Shots".into()));
        assert_eq!(expand("~"), Ok("/home/me".into()));
        assert_eq!(expand("Shots"), Ok("/home/me/Shots".into()));
        assert_eq!(expand("/srv/shots"), Ok("/srv/shots".into()));
        assert_eq!(expand("$SHOTS/today"), Ok("/data/shots/today".into()));
        assert_eq!(expand("${SHOTS}/today"), Ok("/data/shots/today".into()));
        assert!(expand("$NOT_SET/today").is_err());
    }
}

fn rustix_uid() -> u32 {
    // Avoid a libc dependency for one call: the runtime dir owner is good enough.
    std::fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
        .unwrap_or(0)
}
