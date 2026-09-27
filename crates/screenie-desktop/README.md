# screenie-desktop

Fitting screenie into GNOME and KDE Plasma, which have their own ways where other
compositors have the usual Wayland protocols.

| Module | Role |
| --- | --- |
| `Desktop` | Which desktop this is, from `XDG_CURRENT_DESKTOP` (or `KDE_FULL_SESSION`). Desktops built on GNOME's libraries with a shell of their own (Budgie, Cinnamon, Pantheon…) count as other. |
| `entry` | The desktop entry `dev.johnpyp.Screenie.desktop` and its icon, in `$XDG_DATA_HOME`. KWin grants its screenshots and casts to the binary an entry runs, and the portals need an app id with an entry, so `ensure` rewrites it whenever the binary moves, and has KDE rebuild its app database. A packaged entry for the same binary makes it unnecessary. |
| `shortcuts` | `screenie shortcuts`: the desktop's screenshot keys, for screenie. |
| `notify` | Desktop notifications with a thumbnail and buttons: the preview on GNOME, which has no layer-shell for cards. |
| `clipboard` | GNOME's clipboard, through a Mutter remote desktop session that's never started (GNOME has no data-control). |

## Shortcuts

GNOME and KDE bind screenshot keys in their own settings. `install` takes them over
there, following the desktop's layout, and returns what it took; `remove` gives it back,
unless something else has the key by then. Screenie's keys show in the desktop's
shortcut settings, where they can be changed.

| GNOME key | Runs | Taken from |
| --- | --- | --- |
| Print | `shot` | `show-screenshot-ui` |
| Shift+Print | `shot all` | `screenshot` |
| Alt+Print | `shot window` | `screenshot-window` |
| Ctrl+Alt+Shift+R | `record` | `show-screen-recording-ui` |

| KDE key | Runs | Taken from (Spectacle) |
| --- | --- | --- |
| Print, Meta+Shift+S, Meta+Shift+Print | `shot` | Launch, Capture Rectangular Region |
| Shift+Print | `shot all` | Capture Entire Desktop |
| Meta+Print | `shot window` | Capture Active Window |
| Meta+Ctrl+Print | `shot window -i` | Capture Window Under Cursor |
| Meta+Shift+R, Meta+R | `record` | Region Recording |
| Meta+Alt+R | `record screen` | Screen Recording |
| Meta+Ctrl+R | `record window -i` | Window Recording |

- **GNOME**: custom keybindings of gsd-media-keys, at `…/custom-keybindings/screenie-<id>/`,
  all through `gsettings`. A built-in binding wins over a custom one, so the key comes
  out of whichever setting holds it (a `*.keybindings` schema's, media-keys', another
  custom keybinding). A setting given back its default is reset, so it follows GNOME's
  defaults again.
- **KDE**: the entry's actions, with `X-KDE-Shortcuts`, the way apps declare their
  shortcuts. kglobalacceld reads an entry only when it first makes its component, so
  `install` writes the entry and then has the component made afresh, and sets the keys
  with `setForeignShortcutKeys`, as System Settings does. Keys are taken from their
  holders the same way.
- **Elsewhere**, keys are bound in the compositor's configuration: `snippet` has the
  lines for sway, Hyprland and niri.

The keys run a path to `screenie` given by the caller, one that should outlast upgrades
(the CLI's, from `$PATH`), while the entry's own `Exec` is the running binary, as KWin
checks it. The daemon repoints the keys at itself if that path goes away.
