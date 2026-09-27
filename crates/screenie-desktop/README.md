# screenie-desktop

Fitting screenie into GNOME and KDE Plasma, which have their own ways where other
compositors have the usual Wayland protocols.

| Module | Role |
| --- | --- |
| `Desktop` | Which desktop this is, from `XDG_CURRENT_DESKTOP` (or `KDE_FULL_SESSION`). Desktops built on GNOME's libraries with a shell of their own (Budgie, Cinnamon, Pantheon…) count as other. |
| `entry` | The desktop entry `dev.johnpyp.Screenie.desktop` and its icon, in `$XDG_DATA_HOME`. KWin grants its screenshots and casts to the binary an entry runs, and the portals need an app id with an entry, so `ensure` rewrites it whenever the binary moves, and has KDE rebuild its app database. A packaged entry for the same binary makes it unnecessary. |
| `shortcuts` | `screenie shortcuts`: the desktop's screenshot keys, for screenie. |
| `extension` | Screenie's GNOME Shell extension (`extension/`, embedded): installing, enabling, removing it (`screenie extension`), keeping an installed copy current, and how it stands. |
| `shell` | Asking the running extension things (see below). |
| `gsettings` | GNOME's settings, through the `gsettings` tool. |
| `overview` | GNOME's overview takes a window opened over it in as a thumbnail, and it's up after login and still animating out after launching an app from it. Captures leave it first (through `org.gnome.Shell`'s `OverviewActive`), waiting until it's gone. |
| `notify` | Desktop notifications with a thumbnail and buttons: the preview on GNOME, which has no layer-shell for cards. |
| `clipboard` | GNOME's clipboard, through a Mutter remote desktop session that's never started (GNOME has no data-control). |

## The GNOME Shell extension

GNOME lets only code in its shell see where windows are, float windows over others, or
take a screenshot without flashing the screen and asking first. `screenie@johnpyp.dev`
does these for screenie, on the session bus as `dev.johnpyp.Screenie.Shell`, which
answers only the binary screenie's desktop entry runs (and nothing sandboxed), as KWin
grants its screenshots. `Features` says what it does, as GNOME versions differ:

| Feature | What | GNOME |
| --- | --- | --- |
| `windows` | `Windows`: the active workspace's app windows, topmost first, with Mutter's ids (which its casts take) and frames in the logical layout | 46+ |
| `pointer` | `PointerOutput`: which output the pointer is on | 46+ |
| `quiet-casts` | `QuietNextCast`, `CastStarted`: the cast screenie starts in between (a still) isn't shown in the top bar. Mutter makes the cast's handle while starting it, and the top bar's indicators are kept from seeing it | 46+ |
| `overlays` | `Place` for a window that takes the keyboard (the selector, the editor): kept above, focused, with no animation, and GNOME's shortcuts and banners wait while it has the focus | 46+ |
| `floating` | `Place` for one that doesn't (cards, a recording's chrome): a dock, never focused, placed by its anchors | 49+ |

`Place(title, layer)` describes screenie's next window with that title as the
layer-shell surface it stands for (output, anchors, margins, keyboard). Stills are the
first frame of a Mutter cast (fast, exact pixels): the shell's own screenshots come out
only as PNG, which takes seconds on a busy 4K screen.

GNOME loads extensions at login, so one installed or updated mid-session runs from the
next, and GNOME turns them off while the screen is locked: `shell::offers` asks again
every couple of seconds, and whatever it doesn't offer is done as without it.

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

The keys run the path to `screenie` the caller gives: the desktop runs them with its own
`PATH`, which often lacks `~/.cargo/bin` or mise's, so the CLI gives the one the user's
shell found (`~/.cargo/bin/screenie`, mise's `…/latest/bin/screenie`,
`~/.nix-profile/bin/screenie`), which stays put across upgrades. The entry's own `Exec`
is the running binary, as KWin checks it.
