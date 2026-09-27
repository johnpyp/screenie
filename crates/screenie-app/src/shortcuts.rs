//! `screenie shortcuts`: the desktop's screenshot keys, for screenie.
//!
//! GNOME and KDE Plasma bind screenshot keys in their own settings; screenie takes them
//! over there (see `screenie_desktop::shortcuts`) and keeps what it took in the state
//! file, to give back. Elsewhere, keys are bound in the compositor's configuration, and
//! this prints the lines to add.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use gpui::AsyncApp;
use screenie_desktop::Desktop;
use screenie_desktop::shortcuts::{self, Taken};
use screenie_ipc::{Response, ShortcutsAction};
use screenie_state::{Shortcuts, State, StateFile, TakenKey};

use crate::daemon::Daemon;

pub(crate) async fn handle(
    action: ShortcutsAction,
    command: PathBuf,
    cx: &mut AsyncApp,
) -> Response {
    let (state, compositor) = cx.update(|cx| {
        let d = Daemon::get(cx);
        (d.state.clone(), d.capture.compositor().name())
    });
    let desktop = Desktop::current();
    // gsettings and D-Bus calls, off the main thread.
    let result = cx
        .background_executor()
        .spawn(async move { run(action, desktop, &command, &state, compositor) })
        .await;
    match result {
        Ok(text) => Response::Text { text },
        Err(e) => Response::error(e),
    }
}

fn run(
    action: ShortcutsAction,
    desktop: Desktop,
    command: &Path,
    state: &StateFile,
    compositor: &str,
) -> Result<String, String> {
    if desktop == Desktop::Other {
        let lines = configuration(compositor, command);
        return match action {
            ShortcutsAction::Show => Ok(lines),
            ShortcutsAction::Install => Err(format!(
                "there are no desktop shortcut settings to install into here (only GNOME \
                 and KDE Plasma have them).\n\n{lines}"
            )),
            ShortcutsAction::Remove => Ok("Nothing to remove: keys are bound in the \
                compositor's configuration here."
                .into()),
        };
    }
    match action {
        ShortcutsAction::Show => {}
        ShortcutsAction::Install => install(state, desktop, command)?,
        ShortcutsAction::Remove => {
            let taken: Vec<Taken> = installed_now(state, desktop)
                .into_iter()
                .flat_map(|i| i.taken)
                .map(|t| Taken {
                    from: t.from,
                    key: t.key,
                })
                .collect();
            shortcuts::remove(desktop, &taken).map_err(|e| e.to_string())?;
            save(state, desktop, None)?;
        }
    }
    describe(desktop, installed_now(state, desktop).as_ref())
}

/// Bind the keys to `command`, and remember what was taken for them, along with what
/// an earlier install took (still to give back).
fn install(state: &StateFile, desktop: Desktop, command: &Path) -> Result<(), String> {
    let taken = shortcuts::install(desktop, command).map_err(|e| e.to_string())?;
    let mut all = installed_now(state, desktop)
        .map(|i| i.taken)
        .unwrap_or_default();
    all.extend(taken.into_iter().map(|t| TakenKey {
        from: t.from,
        key: t.key,
    }));
    let record = Shortcuts {
        command: command.to_path_buf(),
        taken: all,
    };
    save(state, desktop, Some(record))
}

/// Screenie's keys on `desktop`, if they're installed.
pub(crate) fn installed(state: &State, desktop: Desktop) -> Option<&Shortcuts> {
    match desktop {
        Desktop::Gnome => state.shortcuts.gnome.as_ref(),
        Desktop::Kde => state.shortcuts.kde.as_ref(),
        Desktop::Other => None,
    }
}

fn installed_now(state: &StateFile, desktop: Desktop) -> Option<Shortcuts> {
    installed(&state.state(), desktop).cloned()
}

fn save(state: &StateFile, desktop: Desktop, record: Option<Shortcuts>) -> Result<(), String> {
    state
        .update(|s| match desktop {
            Desktop::Gnome => s.shortcuts.gnome = record,
            Desktop::Kde => s.shortcuts.kde = record,
            Desktop::Other => {}
        })
        .map_err(|e| e.to_string())
}

/// Screenie's keys on `desktop`, and what holds each now.
fn describe(desktop: Desktop, installed: Option<&Shortcuts>) -> Result<String, String> {
    let keys = shortcuts::keys(desktop).map_err(|e| e.to_string())?;
    let mut out = String::new();
    let _ = match installed {
        Some(i) if !i.command.is_file() => writeln!(
            out,
            "{}'s screenshot keys, for screenie: installed, but they run {}, which is \
             gone.\n`screenie shortcuts install` points them at this screenie.\n",
            desktop.name(),
            i.command.display()
        ),
        Some(i) => writeln!(
            out,
            "{}'s screenshot keys, for screenie: installed, running {}.\n",
            desktop.name(),
            i.command.display()
        ),
        None => writeln!(
            out,
            "{}'s screenshot keys, for screenie: not installed.\n",
            desktop.name()
        ),
    };
    let rows: Vec<[String; 3]> = keys
        .iter()
        .map(|key| {
            let held = match key.holders.as_slice() {
                [] => "nothing".to_string(),
                holders => holders
                    .iter()
                    .map(|h| {
                        if h.screenie {
                            "screenie".to_string()
                        } else {
                            h.name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            [
                key.chord.label(desktop),
                format!("screenie {}", key.binding.args.join(" ")),
                held,
            ]
        })
        .collect();
    let header = ["Key".to_string(), "Runs".into(), "Held by".into()];
    let width = |i: usize| {
        std::iter::once(&header)
            .chain(&rows)
            .map(|r| r[i].chars().count())
            .max()
            .unwrap_or(0)
    };
    let (w0, w1) = (width(0), width(1));
    for [key, runs, held] in std::iter::once(&header).chain(&rows) {
        let _ = writeln!(out, "  {key:w0$}  {runs:w1$}  {held}");
    }
    let settings = match desktop {
        Desktop::Kde => "System Settings → Keyboard → Shortcuts → Screenie",
        _ => "Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts",
    };
    let _ = match installed {
        Some(_) => write!(
            out,
            "\nChange them in {settings}.\n`screenie shortcuts remove` gives them back."
        ),
        None => write!(
            out,
            "\n`screenie shortcuts install` takes them from what holds them now, and\n\
             `screenie shortcuts remove` gives them back."
        ),
    };
    Ok(out)
}

/// The lines binding screenie's keys in the compositor's configuration.
fn configuration(compositor: &str, command: &Path) -> String {
    const FILES: &[(&str, &str, &str)] = &[
        ("sway", "sway", "~/.config/sway/config"),
        ("hyprland", "Hyprland", "~/.config/hypr/hyprland.conf"),
        ("niri", "niri", "~/.config/niri/config.kdl"),
    ];
    let command = command.to_string_lossy();
    match FILES.iter().find(|(id, ..)| *id == compositor) {
        Some((id, name, file)) => format!(
            "Keys are bound in {name}'s configuration. Screenie's, for {file}:\n\n{}",
            shortcuts::snippet(id, &command).trim_end()
        ),
        None => {
            let mut out = String::from(
                "Keys are bound in the compositor's configuration. Screenie's, for sway, \
                 Hyprland and niri:",
            );
            for (id, _, file) in FILES {
                let _ = write!(
                    out,
                    "\n\n# {file}\n{}",
                    shortcuts::snippet(id, &command).trim_end()
                );
            }
            out
        }
    }
}
