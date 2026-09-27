//! GNOME: screenie's keys are custom keybindings of gnome-settings-daemon, which GNOME
//! Settings lists under Custom Shortcuts. A built-in binding wins over a custom one, so
//! the keys are first taken from GNOME's own (its screenshot UI's).

use std::path::Path;

use super::{Binding, Chord, Holder, Key, Result, Taken, shell_quote};
use crate::gsettings::{self, Value};

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
/// Relocatable: one per custom keybinding, at a path listed in `custom-keybindings`.
const CUSTOM: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const CUSTOM_DIR: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";
/// Screenie's custom keybindings are at `CUSTOM_DIR/screenie-<id>/`.
const OURS: &str = "screenie-";

/// A setting that binds keys: a built-in binding, or a custom one's `binding`.
#[derive(Debug, Clone)]
struct Setting {
    /// With a custom keybinding's path: `schema:/path/`.
    schema: String,
    key: String,
    value: Value,
    /// What it's called, for people.
    name: String,
    screenie: bool,
    changed: bool,
}

impl Setting {
    fn accelerators(&self) -> &[String] {
        match &self.value {
            Value::List(list) => list,
            Value::One(one) => std::slice::from_ref(one),
        }
    }

    fn holds(&self, chord: &Chord) -> bool {
        self.accelerators()
            .iter()
            .any(|a| parse_accelerator(a).as_ref() == Some(chord))
    }

    /// Unbind `chord`: the accelerators it was bound as.
    fn take(&mut self, chord: &Chord) -> Vec<String> {
        let held = |a: &String| parse_accelerator(a).as_ref() == Some(chord);
        let taken: Vec<String> = self
            .accelerators()
            .iter()
            .filter(|a| held(a))
            .cloned()
            .collect();
        match &mut self.value {
            Value::List(list) => list.retain(|a| !held(a)),
            Value::One(one) => one.clear(),
        }
        self.changed |= !taken.is_empty();
        taken
    }

    fn give(&mut self, accelerator: &str) {
        match &mut self.value {
            Value::List(list) => list.push(accelerator.to_string()),
            Value::One(one) => *one = accelerator.to_string(),
        }
        self.changed = true;
    }

    fn save(&self) -> Result<()> {
        Ok(gsettings::set(&self.schema, &self.key, &self.value)?)
    }

    /// Save, as the default if it's that again (most likely what it was before screenie
    /// took from it), so it follows the desktop's defaults as it did.
    fn restore(&self) -> Result<()> {
        gsettings::run(&["reset", &self.schema, &self.key])?;
        let default = gsettings::get(&self.schema, &self.key)?;
        let sorted = |v: &Value| {
            let mut all = match v {
                Value::List(list) => list.clone(),
                Value::One(one) => vec![one.clone()],
            };
            all.sort();
            all
        };
        if default.is_some_and(|d| sorted(&d) == sorted(&self.value)) {
            return Ok(());
        }
        self.save()
    }
}

/// Every setting that binds keys, built in or custom.
fn settings() -> Result<Vec<Setting>> {
    let mut settings = Vec::new();
    for line in gsettings::run(&["list-recursively"])?.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(schema), Some(key), Some(value)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let bindings = schema.ends_with(".keybindings") || schema == MEDIA_KEYS;
        if !bindings || key == "custom-keybindings" {
            continue;
        }
        let Some(value) = Value::parse(value) else {
            continue;
        };
        settings.push(Setting {
            schema: schema.to_string(),
            key: key.to_string(),
            value,
            name: format!("GNOME's {key}"),
            screenie: false,
            changed: false,
        });
    }
    for path in custom_paths()? {
        let schema = format!("{CUSTOM}:{path}");
        let get = |key| gsettings::get_string(&schema, key);
        let (Some(binding), name) = (get("binding")?, get("name")?) else {
            continue;
        };
        settings.push(Setting {
            name: format!("custom shortcut “{}”", name.unwrap_or_default()),
            screenie: is_ours(&path),
            schema,
            key: "binding".into(),
            value: Value::One(binding),
            changed: false,
        });
    }
    Ok(settings)
}

fn custom_paths() -> Result<Vec<String>> {
    Ok(gsettings::get_list(MEDIA_KEYS, "custom-keybindings")?)
}

/// The list of custom keybindings, as `paths`.
fn custom_list(paths: Vec<String>) -> Setting {
    Setting {
        schema: MEDIA_KEYS.into(),
        key: "custom-keybindings".into(),
        value: Value::List(paths),
        name: String::new(),
        screenie: false,
        changed: true,
    }
}

fn is_ours(path: &str) -> bool {
    path.strip_prefix(CUSTOM_DIR)
        .is_some_and(|name| name.starts_with(OURS))
}

pub(super) fn keys(layout: &'static [Binding]) -> Result<Vec<Key>> {
    let settings = settings()?;
    Ok(super::each_key(layout)
        .map(|(binding, chord)| {
            let holders = settings
                .iter()
                .filter(|s| s.holds(&chord))
                .map(|s| Holder {
                    name: s.name.clone(),
                    screenie: s.screenie,
                })
                .collect();
            Key {
                binding,
                chord,
                holders,
            }
        })
        .collect())
}

pub(super) fn install(layout: &'static [Binding], command: &Path) -> Result<Vec<Taken>> {
    let mut settings = settings()?;
    let mut taken = Vec::new();
    for (_, chord) in super::each_key(layout) {
        for setting in settings.iter_mut().filter(|s| !s.screenie) {
            for accelerator in setting.take(&chord) {
                taken.push(Taken {
                    from: vec![setting.schema.clone(), setting.key.clone()],
                    key: accelerator,
                });
            }
        }
    }
    for setting in settings.iter().filter(|s| s.changed) {
        setting.save()?;
    }

    let command = shell_quote(&command.to_string_lossy());
    let mut ours = Vec::new();
    for binding in layout {
        for (i, key) in binding.keys.iter().enumerate() {
            let chord = Chord::parse(key).expect("layouts parse");
            let id = match i {
                0 => binding.id.to_string(),
                i => format!("{}-{}", binding.id, i + 1),
            };
            let path = format!("{CUSTOM_DIR}{OURS}{id}/");
            let schema = format!("{CUSTOM}:{path}");
            let args: Vec<String> = binding.args.iter().map(|a| shell_quote(a)).collect();
            let run = format!("{command} {}", args.join(" "));
            let name = format!("Screenie: {}", binding.name);
            gsettings::set(&schema, "name", &Value::One(name))?;
            gsettings::set(&schema, "command", &Value::One(run))?;
            gsettings::set(&schema, "binding", &Value::One(accelerator(&chord)))?;
            ours.push(path);
        }
    }
    // Ours after the user's own, and none left over from another layout.
    let listed = custom_paths()?;
    for path in listed.iter().filter(|p| is_ours(p) && !ours.contains(p)) {
        reset(path)?;
    }
    let mut paths: Vec<String> = listed.into_iter().filter(|p| !is_ours(p)).collect();
    paths.extend(ours);
    custom_list(paths).save()?;
    Ok(taken)
}

pub(super) fn remove(taken: &[Taken]) -> Result<()> {
    let (ours, theirs): (Vec<String>, Vec<String>) =
        custom_paths()?.into_iter().partition(|p| is_ours(p));
    custom_list(theirs).restore()?;
    for path in &ours {
        reset(path)?;
    }

    let mut settings = settings()?;
    for t in taken {
        let [schema, key] = t.from.as_slice() else {
            continue;
        };
        let Some(chord) = parse_accelerator(&t.key) else {
            continue;
        };
        // It stays with whatever has it now.
        if let Some(holder) = settings.iter().find(|s| s.holds(&chord)) {
            tracing::info!(
                "not giving {} back to {schema} {key}: {} has it",
                t.key,
                holder.name
            );
            continue;
        }
        // A custom shortcut since deleted has nothing to take it back.
        if let Some(setting) = settings
            .iter_mut()
            .find(|s| &s.schema == schema && &s.key == key)
        {
            setting.give(&t.key);
        }
    }
    for setting in settings.iter().filter(|s| s.changed) {
        setting.restore()?;
    }
    Ok(())
}

/// Forget a custom keybinding's settings.
fn reset(path: &str) -> Result<()> {
    Ok(gsettings::run(&["reset-recursively", &format!("{CUSTOM}:{path}")]).map(drop)?)
}

/// `chord` as GTK writes accelerators: `<Ctrl><Shift>Print`.
fn accelerator(chord: &Chord) -> String {
    let mut out = String::new();
    for (on, name) in [
        (chord.logo, "<Super>"),
        (chord.ctrl, "<Ctrl>"),
        (chord.alt, "<Alt>"),
        (chord.shift, "<Shift>"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&chord.key);
    out
}

/// A GTK accelerator, if it's one [`Chord`] can say.
fn parse_accelerator(text: &str) -> Option<Chord> {
    let mut chord = Chord::default();
    let mut rest = text.trim();
    while let Some(tail) = rest.strip_prefix('<') {
        let (modifier, tail) = tail.split_once('>')?;
        match modifier.to_ascii_lowercase().as_str() {
            "shift" => chord.shift = true,
            "control" | "ctrl" | "ctl" | "primary" => chord.ctrl = true,
            "alt" | "mod1" => chord.alt = true,
            "super" | "mod4" => chord.logo = true,
            _ => return None,
        }
        rest = tail;
    }
    if rest.is_empty() || rest == "disabled" {
        return None;
    }
    chord.key = super::normal_key(rest);
    Some(chord)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators_read_the_way_gtk_writes_them() {
        let chord = parse_accelerator("<Ctrl><Shift><Alt>R").unwrap();
        assert_eq!(Some(chord.clone()), Chord::parse("Ctrl+Alt+Shift+R"));
        assert_eq!(parse_accelerator("<Control><Shift><Alt>r"), Some(chord));
        assert_eq!(parse_accelerator("Print"), Chord::parse("Print"));
        assert_eq!(parse_accelerator("<Hyper>Print"), None);
        assert_eq!(parse_accelerator(""), None);
        let super_s = Chord::parse("Super+Shift+S").unwrap();
        assert_eq!(accelerator(&super_s), "<Super><Shift>S");
        assert_eq!(parse_accelerator(&accelerator(&super_s)), Some(super_s));
    }

    #[test]
    fn only_screenies_custom_keybindings_are_its_own() {
        assert!(is_ours(&format!("{CUSTOM_DIR}screenie-shot/")));
        assert!(!is_ours(&format!("{CUSTOM_DIR}custom0/")));
    }
}
