//! Binding screenie to the desktop's screenshot keys, the way the desktop has them.
//!
//! GNOME and KDE Plasma bind screenshots to Print and friends themselves, each its own
//! way. [`layout`] follows theirs, so the keys do what they did, only with screenie.
//! [`install`] takes the keys from whatever holds them (GNOME Shell's screenshot UI,
//! Spectacle) and binds screenie in the desktop's own shortcut settings, where the user
//! can change them; it returns what it took, which [`remove`] gives back. Compositors
//! configured in a file get the lines to add instead ([`snippet`]).

mod gnome;
mod kde;

use std::path::Path;

use crate::Desktop;

/// A key binding: a command screenie offers, and the keys it's bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Its id: a desktop entry action (KDE), part of a custom keybinding's path (GNOME).
    pub id: &'static str,
    /// What it does, as the desktop's shortcut settings show it.
    pub name: &'static str,
    /// Arguments to `screenie`.
    pub args: &'static [&'static str],
    /// Each a [`Chord`] (`Shift+Print`).
    pub keys: &'static [&'static str],
}

/// GNOME's own: Print opens the screenshot UI, Shift takes the whole desktop, Alt the
/// window, and the screen recorder has a chord of its own.
const GNOME: &[Binding] = &[
    Binding {
        id: "shot",
        name: "Screenshot",
        args: &["shot"],
        keys: &["Print"],
    },
    Binding {
        id: "shot-all",
        name: "Screenshot of the desktop",
        args: &["shot", "all"],
        keys: &["Shift+Print"],
    },
    Binding {
        id: "shot-window",
        name: "Screenshot of the window",
        args: &["shot", "window"],
        keys: &["Alt+Print"],
    },
    Binding {
        id: "record",
        name: "Record the screen",
        args: &["record"],
        keys: &["Ctrl+Alt+Shift+R"],
    },
];

/// Spectacle's: Print and Meta+Shift+S pick a region, Shift takes the desktop, Meta the
/// active window, Meta+Ctrl the one you point at, and Meta+R records.
const KDE: &[Binding] = &[
    Binding {
        id: "shot",
        name: "Screenshot",
        args: &["shot"],
        keys: &["Print", "Super+Shift+S", "Super+Shift+Print"],
    },
    Binding {
        id: "shot-all",
        name: "Screenshot of the entire desktop",
        args: &["shot", "all"],
        keys: &["Shift+Print"],
    },
    Binding {
        id: "shot-window",
        name: "Screenshot of the active window",
        args: &["shot", "window"],
        keys: &["Super+Print"],
    },
    Binding {
        id: "shot-pick-window",
        name: "Screenshot of a window",
        args: &["shot", "window", "-i"],
        keys: &["Super+Ctrl+Print"],
    },
    Binding {
        id: "record",
        name: "Record",
        args: &["record"],
        keys: &["Super+Shift+R", "Super+R"],
    },
    Binding {
        id: "record-screen",
        name: "Record the screen",
        args: &["record", "screen"],
        keys: &["Super+Alt+R"],
    },
    Binding {
        id: "record-window",
        name: "Record a window",
        args: &["record", "window", "-i"],
        keys: &["Super+Ctrl+R"],
    },
];

/// Tiling compositors': screenie's own.
const OTHER: &[Binding] = &[
    Binding {
        id: "shot",
        name: "Screenshot",
        args: &["shot"],
        keys: &["Print"],
    },
    Binding {
        id: "shot-screen",
        name: "Screenshot of the screen",
        args: &["shot", "screen"],
        keys: &["Shift+Print"],
    },
    Binding {
        id: "record",
        name: "Record",
        args: &["record"],
        keys: &["Alt+Print"],
    },
];

/// The bindings for `desktop`, following its own.
pub fn layout(desktop: Desktop) -> &'static [Binding] {
    match desktop {
        Desktop::Gnome => GNOME,
        Desktop::Kde => KDE,
        Desktop::Other => OTHER,
    }
}

/// A key with modifiers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The logo key: Super, or Meta as KDE calls it.
    pub logo: bool,
    /// Its name, as xkb has it: `Print`, a letter.
    pub key: String,
}

impl Chord {
    /// Read `Ctrl+Alt+Shift+Super+Key` (in any order, any case).
    pub fn parse(text: &str) -> Option<Chord> {
        let mut chord = Chord::default();
        let mut parts = text.split('+').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                chord.key = normal_key(part);
                break;
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "primary" => chord.ctrl = true,
                "alt" => chord.alt = true,
                "shift" => chord.shift = true,
                "super" | "meta" | "logo" | "mod4" => chord.logo = true,
                _ => return None,
            }
        }
        (!chord.key.is_empty()).then_some(chord)
    }

    /// As `desktop` shows it: `Super+Shift+S`, or `Meta+Shift+S` on KDE.
    pub fn label(&self, desktop: Desktop) -> String {
        let logo = if desktop == Desktop::Kde {
            "Meta"
        } else {
            "Super"
        };
        let mut parts = Vec::new();
        for (on, name) in [
            (self.logo, logo),
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// Keys are compared by name, starting with a capital: `Print`, `S`, `F1`.
fn normal_key(key: &str) -> String {
    let mut chars = key.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Each of `layout`'s keys, with its binding.
pub(crate) fn each_key(
    layout: &'static [Binding],
) -> impl Iterator<Item = (&'static Binding, Chord)> {
    layout.iter().flat_map(|binding| {
        binding
            .keys
            .iter()
            .map(move |key| (binding, Chord::parse(key).expect("layouts parse")))
    })
}

/// A key screenie took from a shortcut of the desktop's own, to give back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// Where it was: a GNOME setting (schema and key), a KDE action (component and
    /// action).
    pub from: Vec<String>,
    /// The key, in that desktop's notation.
    pub key: String,
}

/// One of a layout's keys, and what holds it now.
#[derive(Debug, Clone)]
pub struct Key {
    pub binding: &'static Binding,
    pub chord: Chord,
    pub holders: Vec<Holder>,
}

/// Something that holds a key now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// What it's called: "GNOME Shell: show-screenshot-ui", "Spectacle: Capture
    /// Rectangular Region".
    pub name: String,
    /// It's screenie.
    pub screenie: bool,
}

/// Something went wrong talking to the desktop's settings.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Each of `desktop`'s keys, and what holds it now.
pub fn keys(desktop: Desktop) -> Result<Vec<Key>> {
    match desktop {
        Desktop::Gnome => gnome::keys(GNOME),
        Desktop::Kde => kde::keys(KDE),
        Desktop::Other => Err(Error("there's no desktop to ask".into())),
    }
}

/// Bind `desktop`'s keys to screenie, run as `command`, taking each from whatever holds
/// it. What was taken, for [`remove`].
pub fn install(desktop: Desktop, command: &Path) -> Result<Vec<Taken>> {
    match desktop {
        Desktop::Gnome => gnome::install(GNOME, command),
        Desktop::Kde => kde::install(KDE, command),
        Desktop::Other => Err(Error(
            "keys are bound in the compositor's own configuration here".into(),
        )),
    }
}

/// Unbind screenie, and give back what [`install`] took (unless something else has
/// taken it since).
pub fn remove(desktop: Desktop, taken: &[Taken]) -> Result<()> {
    match desktop {
        Desktop::Gnome => gnome::remove(taken),
        Desktop::Kde => kde::remove(taken),
        Desktop::Other => Ok(()),
    }
}

/// The desktop entry's actions while screenie's keys are installed on `desktop`, for
/// [`entry::ensure`](crate::entry::ensure): on KDE, they're what the keys run.
pub fn entry_actions(desktop: Desktop) -> Vec<crate::entry::Action> {
    match desktop {
        Desktop::Kde => kde::actions(KDE),
        _ => crate::entry::default_actions(),
    }
}

/// The lines binding screenie's keys in the configuration of `compositor` (as
/// `screenie_compositor` names it: `sway`, `hyprland`, `niri`), run as `command`.
pub fn snippet(compositor: &str, command: &str) -> String {
    let mut out = String::new();
    for (binding, chord) in each_key(OTHER) {
        // sway and Hyprland run commands through a shell; niri takes the arguments.
        let shell = std::iter::once(command)
            .chain(binding.args.iter().copied())
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ");
        let line = match compositor {
            "hyprland" => {
                let mods: Vec<&str> = [
                    (chord.logo, "SUPER"),
                    (chord.ctrl, "CTRL"),
                    (chord.alt, "ALT"),
                    (chord.shift, "SHIFT"),
                ]
                .into_iter()
                .filter_map(|(on, name)| on.then_some(name))
                .collect();
                format!("bind = {}, {}, exec, {shell}", mods.join(" "), chord.key)
            }
            "niri" => {
                let spawn: Vec<String> = std::iter::once(command)
                    .chain(binding.args.iter().copied())
                    .map(|a| format!("{a:?}"))
                    .collect();
                format!(
                    "    {} {{ spawn {}; }}",
                    chord.label(Desktop::Other),
                    spawn.join(" ")
                )
            }
            _ => format!(
                "bindsym {} exec {shell}",
                chord.label(Desktop::Other).replace("Super", "Mod4")
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    if compositor == "niri" {
        out = format!("binds {{\n{out}}}\n");
    }
    out
}

/// `word` for a shell's command line, quoted if it needs it.
pub(crate) fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_read_any_way_round() {
        let chord = Chord::parse("Ctrl+Alt+Shift+R").unwrap();
        assert!(chord.ctrl && chord.alt && chord.shift && !chord.logo);
        assert_eq!(chord.key, "R");
        assert_eq!(Chord::parse("shift+ctrl+alt+r"), Some(chord));
        let meta = Chord::parse("Meta+Shift+print").unwrap();
        assert_eq!(meta.label(Desktop::Gnome), "Super+Shift+Print");
        assert_eq!(meta.label(Desktop::Kde), "Meta+Shift+Print");
        assert_eq!(Chord::parse("Hyper+Print"), None);
    }

    #[test]
    fn every_layout_parses() {
        for desktop in [Desktop::Gnome, Desktop::Kde, Desktop::Other] {
            for binding in layout(desktop) {
                for key in binding.keys {
                    assert!(Chord::parse(key).is_some(), "{key}");
                }
            }
        }
    }

    #[test]
    fn snippets_speak_each_compositors_language() {
        let sway = snippet("sway", "/opt/my tools/screenie");
        assert!(
            sway.contains("bindsym Shift+Print exec '/opt/my tools/screenie' shot screen\n"),
            "{sway}"
        );
        let hyprland = snippet("hyprland", "screenie");
        assert!(
            hyprland.contains("bind = , Print, exec, screenie shot\n"),
            "{hyprland}"
        );
        assert!(
            hyprland.contains("bind = ALT, Print, exec, screenie record\n"),
            "{hyprland}"
        );
        let niri = snippet("niri", "screenie");
        assert!(
            niri.contains("    Alt+Print { spawn \"screenie\" \"record\"; }\n"),
            "{niri}"
        );
    }
}
