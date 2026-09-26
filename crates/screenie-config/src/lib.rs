//! Screenie's configuration: the schema, where it lives on disk, and how output files are
//! named.
//!
//! The config is a YAML file at `$XDG_CONFIG_HOME/screenie/config.yaml`. It is optional;
//! every setting has a default. The settings window writes it and the daemon reloads it
//! when it changes, so hand edits take effect immediately too.

mod naming;
mod paths;
mod schema;

use std::path::{Path, PathBuf};

pub use naming::{Subject, claim_unique, expand_template, unique_path};
pub use paths::Paths;
pub use schema::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<serde_saphyr::Error>,
    },
    #[error("writing {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("serializing config: {0}")]
    Serialize(#[from] serde_saphyr::SerializeError),
}

const HEADER: &str = "\
# Screenie configuration. Every setting is optional; delete a line to restore its default.
# This file is rewritten by the settings window (`screenie settings`).

";

impl Config {
    /// Load from the default location. A missing file yields the defaults.
    pub fn load() -> Result<Config, Error> {
        Self::load_from(&Paths::get().config_file())
    }

    pub fn load_from(path: &Path) -> Result<Config, Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => parse(&text).map_err(|source| Error::Parse {
                path: path.into(),
                source: Box::new(source),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(Error::Read {
                path: path.into(),
                source,
            }),
        }
    }

    /// Load, falling back to defaults (with a logged warning) if the file is broken. The
    /// daemon uses this so a typo never stops screenshots from working.
    pub fn load_or_default() -> Config {
        Self::load().unwrap_or_else(|e| {
            tracing::warn!("{e}; using default settings");
            Config::default()
        })
    }

    pub fn save(&self) -> Result<(), Error> {
        self.save_to(&Paths::get().config_file())
    }

    /// Write atomically so a reader never sees a half-written file.
    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        let text = format!("{HEADER}{}", serde_saphyr::to_string(self)?);
        let write = |p: &Path| std::fs::write(p, &text);
        let werr = |source| Error::Write {
            path: path.into(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(werr)?;
        }
        let tmp = path.with_extension("yaml.tmp");
        write(&tmp).map_err(werr)?;
        std::fs::rename(&tmp, path).map_err(werr)
    }

    /// The directory screenshots are saved to (see [`paths::expand_user_path`]).
    pub fn screenshot_dir(&self) -> PathBuf {
        resolve_dir(&self.screenshot.directory, || {
            Paths::get().pictures_dir().join("Screenshots")
        })
    }

    /// The directory recordings are saved to (see [`paths::expand_user_path`]).
    pub fn recording_dir(&self) -> PathBuf {
        resolve_dir(&self.recording.directory, || {
            Paths::get().videos_dir().join("Screencasts")
        })
    }
}

/// Parse config text. A file with nothing in it (or only comments) is all defaults.
fn parse(text: &str) -> Result<Config, serde_saphyr::Error> {
    let blank = text
        .lines()
        .all(|l| matches!(l.trim_start().chars().next(), None | Some('#')));
    if blank {
        Ok(Config::default())
    } else {
        serde_saphyr::from_str(text)
    }
}

/// A configured directory, or `default` when it's unset or names a variable that isn't.
fn resolve_dir(configured: &Path, default: impl FnOnce() -> PathBuf) -> PathBuf {
    if configured.as_os_str().is_empty() {
        return default();
    }
    paths::expand_user_path(configured).unwrap_or_else(|e| {
        let default = default();
        tracing::warn!("{e}; using {}", default.display());
        default
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The examples in the top-level README (the full one and the presets) must parse,
    /// and use only real keys (unknown keys are tolerated on load, so a typo there would
    /// otherwise go unnoticed).
    #[test]
    fn readme_examples_are_valid() {
        use serde_json::Value;
        fn check(example: &Value, schema: &Value, path: &str) {
            let (Value::Object(example), Value::Object(schema)) = (example, schema) else {
                return;
            };
            for (key, value) in example {
                let here = format!("{path}{key}");
                let known = schema
                    .get(key)
                    .unwrap_or_else(|| panic!("README uses unknown key {here}"));
                check(value, known, &format!("{here}."));
            }
        }
        let schema = serde_json::to_value(Config::default()).unwrap();

        let readme = include_str!("../../../README.md");
        let examples: Vec<_> = readme
            .split("```yaml\n")
            .skip(1)
            .filter_map(|rest| rest.split("```").next())
            .collect();
        assert!(examples.len() > 1, "README has its yaml examples");
        for example in examples {
            let config =
                parse(example).unwrap_or_else(|e| panic!("README example parses: {e}\n{example}"));
            let _ = config.screenshot_dir();
            let example: Value = serde_saphyr::from_str(example).unwrap();
            check(&example, &schema, "");
        }
    }

    #[test]
    fn partial_config_uses_defaults() {
        let cfg = parse("recording:\n  framerate: 30\n").unwrap();
        assert_eq!(cfg.recording.framerate, Framerate::Fps(30));
        assert_eq!(
            cfg.recording.countdown,
            RecordingConfig::default().countdown
        );
        assert_eq!(cfg.screenshot, ScreenshotConfig::default());
    }

    #[test]
    fn empty_or_comment_only_is_default() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(parse("# nothing yet\n\n").unwrap(), Config::default());
        assert_eq!(parse("{}").unwrap(), Config::default());
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("screenie-config-test-{}", std::process::id()));
        let path = dir.join("config.yaml");
        let mut cfg = Config::default();
        cfg.selector.magnifier = false;
        cfg.recording.after_capture.copy = true;
        cfg.recording.quality = Quality::Lossless;
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), cfg);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ui_scale_is_auto_or_a_factor() {
        assert_eq!(parse("").unwrap().ui_scale, UiScale::Auto);
        assert_eq!(parse("ui_scale: auto").unwrap().ui_scale, UiScale::Auto);
        assert_eq!(
            parse("ui_scale: 1.25").unwrap().ui_scale,
            UiScale::Fixed(1.25)
        );
        assert_eq!(parse("ui_scale: 2").unwrap().ui_scale, UiScale::Fixed(2.0));
        assert!(parse("ui_scale: big").is_err());
        assert!(parse("ui_scale: -1").is_err());
        let text = serde_saphyr::to_string(&Config {
            ui_scale: UiScale::Fixed(1.5),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(parse(&text).unwrap().ui_scale, UiScale::Fixed(1.5));
    }

    #[test]
    fn framerate_is_native_or_a_number() {
        assert_eq!(parse("").unwrap().recording.framerate, Framerate::Fps(60));
        assert_eq!(
            parse("recording: { framerate: native }")
                .unwrap()
                .recording
                .framerate,
            Framerate::Native
        );
        assert!(parse("recording: { framerate: 0 }").is_err());
        assert!(parse("recording: { framerate: fast }").is_err());
        let mut cfg = Config::default();
        cfg.recording.framerate = Framerate::Native;
        assert_eq!(parse(&serde_saphyr::to_string(&cfg).unwrap()).unwrap(), cfg);
    }

    #[test]
    fn resolution_is_native_or_lines() {
        assert_eq!(
            parse("").unwrap().recording.resolution,
            Resolution::Lines(1080)
        );
        assert_eq!(
            parse("recording: { resolution: native }")
                .unwrap()
                .recording
                .resolution,
            Resolution::Native
        );
        assert_eq!(
            parse("recording: { resolution: 720p }")
                .unwrap()
                .recording
                .resolution,
            Resolution::Lines(720)
        );
        assert_eq!(
            parse("recording: { resolution: 4k }")
                .unwrap()
                .recording
                .resolution,
            Resolution::Lines(2160)
        );
        assert!(parse("recording: { resolution: 1080 }").is_err());
        assert!(parse("recording: { resolution: big }").is_err());
        let mut cfg = Config::default();
        cfg.recording.resolution = Resolution::Lines(1440);
        assert_eq!(parse(&serde_saphyr::to_string(&cfg).unwrap()).unwrap(), cfg);
        // A box turned to the capture's orientation.
        assert_eq!(
            Resolution::Lines(1080).bounds(3840, 2160),
            Some((1920, 1080))
        );
        assert_eq!(
            Resolution::Lines(1080).bounds(1000, 3000),
            Some((1080, 1920))
        );
        assert_eq!(Resolution::Native.bounds(3840, 2160), None);
    }

    /// Keys that were dropped keep old files loading: they're just ignored.
    #[test]
    fn removed_keys_still_load() {
        let cfg = parse(
            "selector: { freeze: false }\nadvanced: { daemon_idle_exit: 300 }\n\
             recording: { after_capture: { save: false, edit: true, copy: true } }\n",
        )
        .unwrap();
        assert!(cfg.recording.after_capture.copy);
        assert_eq!(cfg.selector, SelectorConfig::default());
        assert_eq!(cfg.advanced, AdvancedConfig::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let cfg = parse("future_thing: 1\nselector:\n  sparkles: true\n").unwrap();
        assert_eq!(cfg, Config::default());
    }
}
