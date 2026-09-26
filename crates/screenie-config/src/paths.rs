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
            let home = base.as_ref().map(|b| b.home_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("/"));
            let uid = rustix_uid();
            Paths {
                config_dir: base.as_ref().map(|b| b.config_dir().to_path_buf()).unwrap_or_else(|| home.join(".config")).join("screenie"),
                state_dir: base
                    .as_ref()
                    .and_then(|b| b.state_dir().map(Path::to_path_buf))
                    .unwrap_or_else(|| home.join(".local/state"))
                    .join("screenie"),
                runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::temp_dir().join(format!("screenie-{uid}")))
                    .join("screenie"),
                pictures: user.as_ref().and_then(|u| u.picture_dir().map(Path::to_path_buf)),
                videos: user.as_ref().and_then(|u| u.video_dir().map(Path::to_path_buf)),
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

    /// Persistent state: capture history, portal permission tokens.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Per-session files: the daemon socket, temporary captures.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn socket(&self) -> PathBuf {
        // Namespaced by display so two sessions for one user get separate daemons.
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let display = display.rsplit('/').next().unwrap_or("wayland-0").to_string();
        self.runtime_dir.join(format!("{display}.sock"))
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn pictures_dir(&self) -> PathBuf {
        self.pictures.clone().unwrap_or_else(|| self.home.join("Pictures"))
    }

    pub fn videos_dir(&self) -> PathBuf {
        self.videos.clone().unwrap_or_else(|| self.home.join("Videos"))
    }
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => Paths::get().home().join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn rustix_uid() -> u32 {
    // Avoid a libc dependency for one call: the runtime dir owner is good enough.
    std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(0)
}
