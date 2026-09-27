//! KDE Plasma: screenie's keys are the actions of its desktop entry, as an app's are
//! (`X-KDE-Shortcuts`), so System Settings lists them under Screenie. kglobalacceld
//! (running in KWin) reads an entry once, when it first sees it, so the entry is written
//! first and then its component made afresh, the way System Settings adds an app. The
//! keys are taken from whatever holds them (Spectacle) and set with
//! `setForeignShortcutKeys`, as System Settings sets them.

use std::path::Path;

use screenie_core::APP_ID;
use serde::Deserialize;
use zbus::zvariant::{OwnedObjectPath, Type};

use super::{Binding, Chord, Error, Holder, Key, Result, Taken};
use crate::entry;

/// A key sequence as Qt sends it: up to four keys, each a key code with modifier bits.
type Sequence = (Vec<i32>,);

/// Qt's modifier bits.
const SHIFT: i32 = 0x0200_0000;
const CTRL: i32 = 0x0400_0000;
const ALT: i32 = 0x0800_0000;
const META: i32 = 0x1000_0000;
/// `Qt::Key_Print`.
const PRINT: i32 = 0x0100_0009;

/// `KGlobalAccel::MatchType::Equal`.
const EQUAL: i32 = 0;

#[derive(Debug, Deserialize, Type)]
struct ShortcutInfo {
    action: String,
    action_name: String,
    component: String,
    component_name: String,
    _context: String,
    _context_name: String,
    _keys: Vec<i32>,
    _default_keys: Vec<i32>,
}

#[zbus::proxy(
    interface = "org.kde.KGlobalAccel",
    default_service = "org.kde.kglobalaccel",
    default_path = "/kglobalaccel",
    gen_blocking = false
)]
trait KGlobalAccel {
    #[zbus(name = "globalShortcutsByKey")]
    fn global_shortcuts_by_key(
        &self,
        key: &Sequence,
        match_type: &(i32,),
    ) -> zbus::Result<Vec<ShortcutInfo>>;
    #[zbus(name = "shortcutKeys")]
    fn shortcut_keys(&self, action_id: &[&str]) -> zbus::Result<Vec<Sequence>>;
    #[zbus(name = "setForeignShortcutKeys")]
    fn set_foreign_shortcut_keys(&self, action_id: &[&str], keys: &[Sequence]) -> zbus::Result<()>;
    #[zbus(name = "doRegister")]
    fn do_register(&self, action_id: &[&str]) -> zbus::Result<()>;
    #[zbus(name = "unregister")]
    fn unregister(&self, component: &str, action: &str) -> zbus::Result<bool>;
    #[zbus(name = "getComponent")]
    fn get_component(&self, component: &str) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.kde.kglobalaccel.Component",
    default_service = "org.kde.kglobalaccel",
    gen_blocking = false
)]
trait Component {
    #[zbus(name = "cleanUp")]
    fn clean_up(&self) -> zbus::Result<bool>;
}

/// Screenie's component: its desktop entry.
fn component() -> String {
    format!("{APP_ID}.desktop")
}

/// The entry's actions for `layout`, with their keys, running `command`.
pub(crate) fn actions(layout: &[Binding]) -> Vec<entry::Action> {
    layout
        .iter()
        .map(|binding| entry::Action {
            id: binding.id.into(),
            name: binding.name.into(),
            args: binding.args.iter().map(|a| a.to_string()).collect(),
            kde_keys: Some(
                binding
                    .keys
                    .iter()
                    .filter_map(|k| Chord::parse(k))
                    .map(|c| portable(&c))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        })
        .collect()
}

pub(super) fn keys(layout: &'static [Binding]) -> Result<Vec<Key>> {
    block_on(async {
        let kga = connect().await?;
        let mut keys = Vec::new();
        for (binding, chord) in super::each_key(layout) {
            let holders = kga
                .global_shortcuts_by_key(&sequence(&chord)?, &(EQUAL,))
                .await?
                .into_iter()
                .map(|info| Holder {
                    // An app's own action is its launch: just its name.
                    name: if info.action == "_launch" {
                        info.component_name
                    } else {
                        format!("{}: {}", info.component_name, info.action_name)
                    },
                    screenie: info.component == component(),
                })
                .collect();
            keys.push(Key {
                binding,
                chord,
                holders,
            });
        }
        Ok(keys)
    })
}

pub(super) fn install(layout: &'static [Binding], command: &Path) -> Result<Vec<Taken>> {
    block_on(async {
        let kga = connect().await?;
        let ours = component();
        let mut taken = Vec::new();
        for (_, chord) in super::each_key(layout) {
            let key = sequence(&chord)?;
            for info in kga.global_shortcuts_by_key(&key, &(EQUAL,)).await? {
                if info.component == ours {
                    continue;
                }
                let id = [
                    info.component.as_str(),
                    &info.action,
                    &info.component_name,
                    &info.action_name,
                ];
                let mut keys = kga.shortcut_keys(&id).await?;
                keys.retain(|k| !same(k, &key));
                kga.set_foreign_shortcut_keys(&id, &keys).await?;
                taken.push(Taken {
                    from: id.iter().map(|s| s.to_string()).collect(),
                    key: portable(&chord),
                });
            }
        }

        entry::ensure(&actions(layout), Some(command))
            .map_err(|e| Error(format!("writing the desktop entry: {e}")))?;
        // Made afresh from the entry just written.
        forget(&kga).await?;
        kga.do_register(&[&ours, "Screenie", "", ""]).await?;
        kga.unregister(&ours, "").await?;

        // Keys it doesn't take show as held by something else in `screenie shortcuts`.
        for binding in layout {
            let id = [ours.as_str(), binding.id, "Screenie", binding.name];
            let keys = binding
                .keys
                .iter()
                .filter_map(|k| Chord::parse(k))
                .map(|c| sequence(&c))
                .collect::<Result<Vec<_>>>()?;
            kga.set_foreign_shortcut_keys(&id, &keys).await?;
        }
        Ok(taken)
    })
}

pub(super) fn remove(taken: &[Taken]) -> Result<()> {
    block_on(async {
        let kga = connect().await?;
        entry::ensure(&entry::default_actions(), None)
            .map_err(|e| Error(format!("writing the desktop entry: {e}")))?;
        forget(&kga).await?;
        for t in taken {
            let Some(chord) = Chord::parse(&t.key) else {
                continue;
            };
            let key = sequence(&chord)?;
            // It stays with whatever has it now.
            if let Some(holder) = kga
                .global_shortcuts_by_key(&key, &(EQUAL,))
                .await?
                .into_iter()
                .next()
            {
                tracing::info!(
                    "not giving {} back: {}: {} has it",
                    t.key,
                    holder.component_name,
                    holder.action_name
                );
                continue;
            }
            let id: Vec<&str> = t.from.iter().map(String::as_str).collect();
            // Its action may be gone (its app uninstalled).
            let Ok(mut keys) = kga.shortcut_keys(&id).await else {
                continue;
            };
            keys.push(key);
            kga.set_foreign_shortcut_keys(&id, &keys).await?;
        }
        Ok(())
    })
}

async fn connect() -> Result<KGlobalAccelProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    Ok(KGlobalAccelProxy::new(&connection).await?)
}

/// Drop screenie's component, if kglobalacceld has one, and with it its keys.
async fn forget(kga: &KGlobalAccelProxy<'_>) -> Result<()> {
    let Ok(path) = kga.get_component(&component()).await else {
        return Ok(());
    };
    ComponentProxy::builder(kga.inner().connection())
        .path(path)?
        .build()
        .await?
        .clean_up()
        .await?;
    Ok(())
}

fn block_on<T>(f: impl Future<Output = Result<T>>) -> Result<T> {
    async_io::block_on(f)
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error(format!("KDE's global shortcuts: {e}"))
    }
}

impl From<zbus::zvariant::Error> for Error {
    fn from(e: zbus::zvariant::Error) -> Self {
        Error(format!("KDE's global shortcuts: {e}"))
    }
}

/// Whether two sequences are the same keys (Qt pads them with zeros).
fn same(a: &Sequence, b: &Sequence) -> bool {
    let keys = |s: &Sequence| s.0.iter().copied().filter(|&k| k != 0).collect::<Vec<_>>();
    keys(a) == keys(b)
}

/// `chord` as a Qt key sequence.
fn sequence(chord: &Chord) -> Result<Sequence> {
    let key = match chord.key.as_str() {
        "Print" => PRINT,
        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => {
            k.as_bytes()[0].to_ascii_uppercase() as i32
        }
        k => return Err(Error(format!("no Qt key code for {k}"))),
    };
    let modifiers = [
        (chord.shift, SHIFT),
        (chord.ctrl, CTRL),
        (chord.alt, ALT),
        (chord.logo, META),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |bits, (_, bit)| bits | bit);
    Ok((vec![key | modifiers, 0, 0, 0],))
}

/// `chord` as Qt writes it (`QKeySequence::PortableText`): `Meta+Shift+S`.
fn portable(chord: &Chord) -> String {
    let mut parts = Vec::new();
    for (on, name) in [
        (chord.logo, "Meta"),
        (chord.ctrl, "Ctrl"),
        (chord.alt, "Alt"),
        (chord.shift, "Shift"),
    ] {
        if on {
            parts.push(name);
        }
    }
    parts.push(&chord.key);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_qt_key_codes() {
        let code = |text| sequence(&Chord::parse(text).unwrap()).unwrap().0[0];
        assert_eq!(code("Print"), PRINT);
        // What kglobalacceld reports for Spectacle's keys.
        assert_eq!(code("Super+Shift+S"), 301989971);
        assert_eq!(code("Super+Shift+Print"), META | SHIFT | PRINT);
        assert_eq!(code("Super+r"), META | 'R' as i32);
        assert!(sequence(&Chord::parse("F13").unwrap()).is_err());
    }

    #[test]
    fn keys_are_written_as_qt_writes_them() {
        let chord = Chord::parse("Shift+Super+S").unwrap();
        assert_eq!(portable(&chord), "Meta+Shift+S");
        assert_eq!(Chord::parse(&portable(&chord)), Some(chord));
    }

    #[test]
    fn every_kde_key_has_a_code() {
        for (_, chord) in crate::shortcuts::each_key(crate::shortcuts::layout(crate::Desktop::Kde))
        {
            assert!(sequence(&chord).is_ok(), "{chord:?}");
        }
    }
}
