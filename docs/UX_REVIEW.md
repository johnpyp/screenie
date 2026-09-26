# UX review

Decisions for you from the simulated QA pass (2026-09-26).

## How this was made

Six agents each invented 8–14 realistic scenarios for one area, and traced them through the code. The scenarios varied people, goals, apps and setups: your gaming desktop, a laptop at 1.25 scaling, mixed monitors, Hyprland/niri/KDE. A skeptic then checked every finding against the code and weighed whether it was worth doing.

- **Bugs:** clear bugs with one right answer were fixed directly (54 of them). The calls made while fixing them are in [section 9](#9-calls-made-while-fixing-bugs), for you to veto.
- **This doc:** the judgement calls.

Each item gives:
- what happens now;
- the options;
- a recommendation.

Where the finder and the skeptic disagreed, both views are shown. Items are grouped by theme and ordered by how much a real user would feel them. The ids (`shot-ux-1`, …) point into `.cache/qa/ux.json` for the full traces.

Mark each one ✅ / ❌ / ✏️ however you like, and I'll work through them.

## The big ones

1. **Errors are invisible from a keybinding.** Nothing tells you a capture or recording failed ([1.1](#11-errors-and-outcomes-are-invisible-from-a-keybinding)).
2. **The card's trash button permanently deletes the file** ([2.1](#21-trash-means-trash)).
3. **Resizable capture box in the inline editor.** Already agreed; it's next after the bug fixes ([6.1](#61-resize-the-capture-box-while-editing-agreed-queued)).
4. **The overlay editor holds the keyboard for its whole session.** With two monitors, typing meant for the other one fires editor shortcuts, and a plain Enter means Done ([6.2](#62-the-overlay-editor-holds-the-keyboard-across-monitors)).
5. **Per-capture copy/save/edit from inside the selector** ([3.1](#31-choose-copy-save-or-edit-per-capture-from-the-selector)).
6. **Game names.** Captures and pills of Steam games are named `steam_app_1245620` ([4.1](#41-game-captures-are-named-steam_app_1245620)).
7. **Save As goes through the portal.** It can park the editor behind a fullscreen game, and needs a FileChooser portal at all. The alternative is an inline save sheet ([6.3](#63-save-as-an-inline-save-sheet-instead-of-the-portal)).
8. **Recording controls have no keys, and some layouts give no feedback at all** ([7.2](#72-keys-on-the-recording-pill), [7.3](#73-recordings-with-no-chrome-give-no-sign-they-started)).
9. **Redaction has no solid black box**, and its strength follows the arrow size ([6.8](#68-solid-redaction-and-self-sizing-blurpixelate)).

With copy and save off, an unsaved capture lives only in its card, and letting it expire is intended. Nothing here tries to keep those captures around.

---

## 1. Feedback

### 1.1 Errors and outcomes are invisible from a keybinding

`shot-ux-1`, `rec-6`, `cli-ux-1`, `input-14`, `preview-9`. Five agents reported this independently.

**Now:**
- Every failure goes to the CLI's stderr, which sway throws away, or to `daemon.log`.
- There are no notifications or toasts anywhere in the app.
- The outcome looks the same as a keypress that did nothing: a capture that failed, a recording that couldn't start, a config that didn't parse, a copy that didn't reach the clipboard.

**Options:**
- a) A daemon-side HUD toast in the preview-card style, on the output where it happened. It's passive (keyboard none, click-through) and bounded, so it's fine over a game.
- b) Freedesktop notifications (`org.freedesktop.Notifications`, via zbus): mako, dunst, swaync, KDE and GNOME all show them.
- c) Both: the toast for errors inside a flow, and a CLI-side notification when the daemon is unreachable or didn't start and stderr isn't a terminal.

**Recommendation:** (c). Both reviewers agreed on the details:
- The toast does most of the work and needs no notification daemon.
- The notification covers "the daemon never answered".
- Benign refusals ("already in progress") stay silent.
- Add `config_error` to `status --json` too.

**Follow-ons if you pick this:**
- A *failed* state on preview cards: a red tint and a short message ("Couldn't finish the recording", "Copy failed: no clipboard access"), shown until dismissed.
- A "Stopped early: disk full" badge on salvaged recordings.
- A toast when recording falls back from window capture to the window's area (`rec-18`).

### 1.2 No confirmation after the editor closes

`editor-11`

**Now:** Done, `exit_on_copy` and `exit_on_save` close the editor silently. The Copied/Saved toasts render inside the editor, which has just closed.

**Options:**
- a) A passive, click-through pill on the capture's output for about 1.2 s: "Copied", "Saved screenshot-….png", or the error.
- b) A freedesktop notification for saves, with "Show in folder".
- c) Nothing.

**Recommendation:** (a), shown only once the work has actually finished, using the same toast as 1.1. The skeptic advised against notifications here, since you've preferred in-app HUD feedback.

### 1.3 "Saved" doesn't say where

`preview-15`

**Now:** the Saved pill on a card has no path.

**Recommendation:** a tooltip with the full path (with `~` for home), and clicking the pill shows the file in its folder, without dismissing the card.

---

## 2. Destructive actions

### 2.1 Trash means trash

`preview-12`, `input-12`

**Now:** the card's trash button and its Delete key call `remove_file`: permanent, with no undo. The icon is a trash can, and you call it "Trash".

**Recommendation:**
- Move the file to the freedesktop trash with the `trash` crate, and title the button "Move to Trash".
- If trashing fails (no trash on that filesystem), keep the file and say so rather than deleting it.
- An undo toast is optional, since the trash is recoverable.

### 2.2 Clicking a card opens the file and removes the card

`preview-13`

**Now:**
- Clicking the card opens the capture in your image viewer and dismisses the card, so copying, saving or editing it afterwards means finding the file again.
- "Show in folder" dismisses it too.

**Options:**
- a) Keep the card after open or reveal, with a fresh short timer, and delete its temp files when the card goes.
- b) Make click open the editor, CleanShot-style, with Open as a button.
- c) Double-click to open.

**Recommendation:** (a). Also add Enter as the Open key.

### 2.3 Discard on the recording pill is one click from Stop

`rec-7`

**Now:** Discard deletes the recording immediately. It's about 30 px from Stop.

**Recommendation:**
- Make it two-step while recording: the first click turns it into a red "Discard?" for 3 s.
- Recordings under ~5 s can stay one click, for false starts.
- `screenie cancel` stays immediate.

### 2.4 `screenie quit` drops unsaved annotations

`cli-ux-14`

**Now:** quit closes open editors without their discard prompt, and the upgrade message recommends running it.

**Recommendation:** refuse with exit 2 and name what's open, e.g. "an editor has unsaved annotations; `screenie quit --force` discards them".

---

## 3. The selector

### 3.1 Choose copy, save or edit per capture, from the selector

`shot-ux-2`

**Now:** per-capture choices exist only as CLI flags baked into the keybinding.

**Options:**
- a) Confirm variants: Ctrl+C captures and copies, Ctrl+S saves, E edits. They add to the configured actions, and the Capture button's tooltip lists them.
- b) Toggles in the screenshot toolbar (Copy, Save, Edit, Cursor), like recording's audio toggles, remembered in state.yaml.

**Recommendation:** (a) now, since it's keyboard-first and adds no chrome. (b) later, if it proves hard to discover.

### 3.2 What Enter means in area mode

`shot-ux-3`

**Now:**
- In area mode with window snapping, Enter confirms whatever is highlighted, which on a tiling layout is almost always a window.
- The Screen button's tooltip still says "Capture this screen  Enter".
- This is the "Enter for the screen" confusion you raised before.

**Options:**
- a) With no selection in area mode, Enter captures the screen under the pointer. Window mode keeps Enter as the highlighted window.
- b) Keep the behaviour and fix the wording (CLI help, North Star, tooltip): "Enter captures what's highlighted".
- c) Enter for what's highlighted, Shift+Enter for the screen.

**Finder:** (a), since each gesture then has one job. **Skeptic:** (b), plus making `3`/`s` capture the screen at once, so the screen is two quick keys away.

### 3.3 Label the confirm button with its target

`rec-9`

**Now:**
- Clicking Record without choosing anything records whatever window the pointer last crossed on its way to the toolbar, often a random terminal.
- That window is shown only by a faint 1.5 px outline.

**Recommendation:**
- Label the button with what it will do right now: "Record Screen", "Record Firefox", "Record 1280×720". Do the same for Capture.
- Optionally make the idle highlight stronger.
- Click and Enter stay identical.

### 3.4 Pressing the shortcut again while the selector is open

`cli-ux-9`, `rec-14`, `input-14`

For screenshots this is done (see [9.1](#91-screenshots)): the same shortcut again closes the selector. For recording:
- **Now:** pressing the record shortcut while the *record* selector is open fails silently, though it toggles at every other stage.
- **Recommendation:** a repeat of the same command closes its selector, after a ~300 ms debounce so a double-tap doesn't cancel. `stop` and `cancel` close an open record selector too. Switching modes with a different command is optional.

### 3.5 Esc and mode keys throw away an adjusted region

`shot-ux-11`

**Now:**
- With `capture_on_release: false`, Esc cancels the whole capture.
- Switching mode away from Area and back loses the region you were adjusting.
- `capture.md` says otherwise.

**Recommendation:**
- Keep Esc as cancel (CleanShot does the same) and fix the doc.
- Remember the Area selection across a mode switch.

### 3.6 Modifiers still held from the shortcut

`shot-ux-8`; the skeptic on `input-16` disagreed.

**Now:** a Shift or Alt still held from `Super+Shift+S` constrains the first drag, making it square or drawn from the centre.

**Finder:** ignore modifiers held when the selector opens, until they're released once. **Skeptic:** not worth it. Modifiers are re-read on every event, so it only lasts while the key is really held, which matches macOS. And the brief wait on close is deliberate: it keeps the key release away from the app underneath.

**Recommendation:** leave it unless it bothers you in practice.

### 3.7 The Window tab without compositor IPC, and `--stdout` in scripts

`shot-ux-13`

**Now:**
- On compositors with no IPC, the Window tab silently behaves like Screen.
- `--stdout` in a script still pops a preview card.

**Recommendation:**
- Show the Window button disabled, with a tooltip "Needs Sway, Hyprland or niri".
- With `--stdout` (or any file output), no card unless asked for. Copy keeps following the config.

### 3.8 The toolbar over game hotbars

`shot-ux-10`: refuted, but worth knowing about.

**Now:** before you draw anything, the toolbar sits at the bottom centre, over a game's hotbar, and swallows presses there.

**Finder:** move it away when the pointer nears. **Skeptic:** that makes the buttons unreachable, because they'd flee as you approach. The only workable variant is click-through until deliberately hovered.

**Recommendation:** no change for now.

---

## 4. Names, files and targets

### 4.1 Game captures are named `steam_app_1245620`

`shot-ux-4`

**Now:** `{app}` in file names, and the window pill, use the raw app id. For Steam games that's `steam_app_<number>`, and for Wine it's `wine`, `java` or `*.exe`.

**Recommendation:**
- Add one shared `display_name(app_id, title)` that uses the cleaned window title when the app id is empty, matches `steam_app_\d+`, or is a generic runtime.
- Use it for `{app}` and the pill.
- For `shot screen` over a fullscreen window, name the file after that window, so Shift+Print in a game gets the game's name.

### 4.2 `-o` and picking a monitor

`cli-ux-5`

**Now:**
- `-o` means output file, but grim users expect `-o OUTPUT` to be a monitor.
- `screenie shot -o DP-1` quietly saves a file named `DP-1` into the daemon's working directory.
- `--output-name` silently turns any target into a whole-screen capture.
- `record` can't pick a monitor at all.

**Recommendation:**
- Take the monitor positionally: `screenie shot screen DP-1` and `screenie record screen DP-1`.
- Keep `--output-name` as a hidden alias that conflicts with non-screen targets.
- Add `.png` or `.mp4` when `-o` has no extension; the capture fix group already did part of this.

### 4.3 A window screenshot is a crop of the screen

`shot-ux-5`

**Now:** picking a window crops the frozen screen, so overlapping windows, notifications and off-screen parts are all as seen. Recording already captures a window by itself (ext-foreign-toplevel, sway 1.11+).

**Options:**
- a) Use one isolated window frame for `shot active`, and for window picks, falling back to the crop.
- b) An opt-in `screenshot.window_capture: isolated` plus a CleanShot-style shadow and padding.

**Finder:** (a) for both. **Skeptic:** (a) for `shot active` only; interactive picks stay what-you-see. Then (b) later. Low priority either way.

---

## 5. Preview cards

### 5.1 Passing over a card steals keyboard focus

`preview-10`

**Now:**
- The card takes the keyboard the instant the pointer enters it. Flying past on the way somewhere else briefly unfocuses your app.
- The gaps between cards hand the keyboard back and forth.

**Options:**
- a) Take the keyboard after a ~120–150 ms dwell. The game pointer-lock rescue still works, because a trapped pointer dwells.
- b) Give it back ~150 ms after the pointer leaves every card, cancelled if it comes back, which fixes the gap churn.
- c) Both.

**Recommendation:** (b) first, then (a) once it's been tried against a fullscreen game with pointer lock on sway.

### 5.2 Show when a card or the pill has the keyboard

`input-10`, `input-17`

**Now:**
- While hovered, a card or the pill silently swallows every key it doesn't handle; the pill handles none at all.
- Your terminal shows as unfocused, and nothing says why typing stopped.

**Finder:** hand the keyboard back on the first key the surface doesn't handle, and draw a focus ring. **Skeptic:** the ring only; handing back on a stray key reopens the pointer-lock trap.

**Recommendation:**
- A subtle ring while `has_keyboard()`, plus an "Esc" keycap on the card.
- Give cards Enter/Space (open) and R (show in folder).
- Give the pill real keys (see 7.2), so holding the keyboard is worth something.

### 5.3 A CLI verb for the newest card

`preview-11`

**Now:** card actions need the mouse.

**Recommendation:** `screenie card copy|save|edit|open|reveal|dismiss|delete` acts on the newest card, plus `dismiss --all`, through the same code as the buttons. Then `bindsym Shift+Print exec screenie card save` works.

### 5.4 Stack behaviour and motion

`preview-14`, `preview-18`

**Now:**
- Only the hovered card pauses its timer, so the stack shifts under the pointer.
- Cards blink out with no animation.
- `top-middle` and `bottom-middle` stack vertically, into the middle of the screen.
- There's no "dismiss all".

**Recommendation:**
- Pause every card's timer while any card is hovered, and give expired ones a short lease when the pointer leaves.
- A ~200 ms slide/fade on removal, later.
- The middle positions lay out as a row.
- Shift+Esc on a card dismisses all.

### 5.5 Tiny captures blur

`preview-16`

**Now:** a 40×20 capture is upscaled into a blurry thumbnail.

**Recommendation:** show small captures at their on-screen size, centred on the card (`ObjectFit::ScaleDown`), as CleanShot does.

---

## 6. Editor

### 6.1 Resize the capture box while editing (agreed, queued)

**Status:** agreed. It will be built after the bug fixes.

**Plan:**
- In the inline (in-place) overlay editor, the editor gets the whole screen's pixels, with your region as the box.
- The box keeps handles the whole time, so you can grow, shrink or move it, like Flameshot.
- Only what's inside is copied or saved.
- Editors opened from a card, `screenie edit FILE`, and regions spanning monitors keep today's shrink-only crop.

**Open question:** an explicit "expand" button when editing from a preview card. It would mean keeping the whole screen with each card, about 33 MB per card at 4K, freed when the card goes. Worth it?

### 6.2 The overlay editor holds the keyboard across monitors

`input-9`

**Now:**
- The editor overlay takes the keyboard exclusively until it closes, however you click elsewhere.
- On a two-monitor setup, typing aimed at the other monitor switches tools and changes sizes, and a plain Enter is Done: it copies, saves and closes.

**Options:**
- a) Open it with on-demand keyboard focus. sway, Hyprland, niri and KWin focus it on map and on click, and give focus away when you click another window.
- b) Keep exclusive focus, but move Done to Ctrl+Enter, or accept plain Enter only after the editor has had pointer input.
- c) Keep exclusive focus, and show a "keys go here" state on the toolbar when the pointer is on another monitor.

**Finder:** (a). **Skeptic:** (b)+(c) for now. (a) needs the keyboard-owner juggling from the input fixes to be solid first.

**Recommendation:** (b) now, since it removes the destructive path cheaply; try (a) later.

### 6.3 Save As: an inline save sheet instead of the portal

`editor-9`

**Now:**
- Ctrl+Shift+S closes the overlay and opens the portal's file chooser, which can land behind a fullscreen app.
- The editor stays parked until the portal answers, and nothing on screen says so.
- With no FileChooser portal installed, Save As can't work at all. It now at least says so, and names the portal packages.

**Options:**
- a) A save sheet inside the overlay:
  - a name field prefilled from the template;
  - a folder picker (screenshot folder, recent folders, Desktop, Downloads);
  - Enter saves;
  - "Browse…" opens the portal only when asked.
- b) Keep the portal, but show a small "Choosing where to save… [Cancel]" pill while parked.

**Recommendation:** (a). It keeps focus where it is, works without portal backends, and is keyboard-first. If the portal stays, at least (b).

### 6.4 The Done button says "Done" whatever it does

`editor-10`

**Now:** what Done will do (copy, save, both, or just close) is only in its tooltip.

**Recommendation:**
- Label it with the action, "Copy", "Save", "Save & copy", or "Done", with a fixed width so the bar doesn't jump.
- Derive the label from the config, so it doesn't flip mid-edit after an explicit Ctrl+C.

### 6.5 One style for every tool, and selecting a shape overwrites it

`editor-12`

**Now:**
- Size and fill are shared by every tool: a thick highlighter makes your next arrow thick.
- Selecting an existing shape copies its style into the style for new shapes, and that's what gets remembered.
- The redaction mode resets to pixelate each session.

**Recommendation:**
- Per-tool size and fill, remembered in state.yaml; colour stays shared.
- Selecting a shape shows its style in the bar without adopting it.
- Remember the last redaction mode.

### 6.6 Switching tools keeps the old selection

`editor-13`

**Now:** after drawing an arrow and pressing R, the arrow stays selected, so the style bar and F act on the arrow, not the rectangle tool.

**Recommendation:**
- Deselect when switching to a drawing tool.
- F, size and colour keys only restyle a selected shape that uses that attribute; otherwise they set the style for new shapes.

### 6.7 Clicking a shape with a drawing tool

`editor-14`

**Now:** with the arrow tool, clicking an existing shape does nothing; you have to press V first.

**Recommendation:** a click without a drag selects the shape under it. A drag still draws, so drawing over shapes keeps working.

### 6.8 Solid redaction, and self-sizing blur/pixelate

`editor-16`

**Now:**
- There are two redaction modes, both sized by the same control as arrows, so thin settings leave text readable.
- There's no opaque black box.

**Recommendation:**
- Add a Solid mode, an opaque box defaulting to black, with B cycling Solid → Pixelate → Blur.
- Size pixelate and blur from the box itself (e.g. block ≥ height/3), independent of the arrow size.
- Whether Solid is the default is your call.

### 6.9 A full-screen in-place edit looks like the live screen

`editor-15`

**Now:** a full-screen capture edited in place is indistinguishable from the real screen, and the bars cover its bottom edge.

**Finder:** apply the centred editor's shrink rule, over 95% → 80%, with a short animation. **Skeptic:** don't shrink. Make the bars move out of the way (to the other edge while you work near them, with hysteresis), and draw a thin accent frame.

**Recommendation:** the skeptic's, since it keeps the in-place feel you wanted. Note that 6.1 will make this case more common.

### 6.10 The close prompt's keys

`editor-18`

**Now:** "Keep your annotations?" only knows Esc (cancel) and Enter (primary).

**Recommendation:**
- D (or Ctrl+D, like macOS "Don't Save") for Discard, and C/S for Copy/Save when shown.
- Show key hints on the buttons.
- Don't make a second Esc discard.

### 6.11 Smaller editor items

- **Renumbering steps** (`editor-19`): when a step is selected, a number stepper in the style bar moves it within the order.
- **Undo granularity** (`editor-20`): merge consecutive arrow-key nudges on the same shape into one undo step. Word-level undo while typing can come later.
- **Shortcut cheat sheet** (`editor-21`): `?` opens a grouped list of every key in a HUD panel, plus a few missing tooltip hints.

---

## 7. Recording

### 7.1 Preview cards and screenshots inside recordings

`rec-8`, `input-18`

**Now (after the fixes):**
- Cards on the recorded output are hidden while an area or screen recording runs. Cards already seen keep expiring as usual; a card from a screenshot taken during the recording waits until it's been seen.
- They're not hidden for window recordings, which only capture the window.
- A screenshot taken during a region recording still records the selector's dim and toolbar.

What's left to decide is the shape of it.

**Options:**
- a) When the countdown starts, hide the card stack on the recorded output and pause its timers. Bring it back after stop, with the new recording's card joining it. Cards from screenshots taken during the recording are queued until it stops.
- b) Move the cards to another output. You said no second-monitor shenanigans.
- c) Place cards clear of the recorded region, reusing the pill-placement logic.

**Recommendation:** keep what's there for screen recordings, and (c) for region recordings, so the cards stay usable beside what's being recorded.

### 7.2 Keys on the recording pill

`rec-10`, `input-13`

**Now:**
- Hovering the pill takes the keyboard, but the pill has no keys, so typing into the recorded app is lost.
- The countdown's stop hint shows literal backticks: "Stop with your record shortcut or \`screenie stop\`".

**Recommendation:**
- Space pauses or resumes, Enter stops and saves, and Esc cancels during the countdown only (never Discard).
- Put the keys in the tooltips, and show a subtle ring while the pill holds the keyboard.
- Reword the hint as plain text: "Press your record shortcut again, or run screenie stop, to finish".

### 7.3 Recordings with no chrome give no sign they started

`rec-11`

**Now:**
- Whole-screen and covered recordings have no pill or ring, by design.
- With `countdown: 0` there's no countdown either, so nothing at all shows that recording started, and there's no stop button.

**Finder:** put the pill on another output when there is one. **Skeptic:** you ruled that out. Instead, when no chrome will show, a short bounded toast as capture starts: "Recording. Press your record shortcut again to stop". Take it down before the first frame, or keep it outside the region.

**Recommendation:** the skeptic's.

### 7.4 Clipping games: `record active`, and screen → fullscreen window

`rec-12`

**Now:**
- `record screen` over a fullscreen game streams the whole output. On wlroots that turns off direct scanout for the whole recording.
- The daemon supports recording the focused window by itself, but the CLI has no `record active`.

**Recommendation:**
- Add `screenie record active` now, and document it as *the* way to clip a game.
- Add `recording.fullscreen_as_window`: when you record a screen one fullscreen window fills, record that window instead.
  - **Finder:** on by default. **Skeptic:** off by default. Your call.
- Note a replay buffer ("save the last 30 s") in the North Star as a later idea.

### 7.5 A resized window drops recording to shared memory for good

`rec-13`

**Now:**
- The first frame of a new size switches a window recording to shared memory for the rest of the recording. At 4K and 60 fps that's the ~2 GB/s read-back that caused your dropped frames.
- The video keeps the first frame's size, so a game going fullscreen from 1280×720 stays 1280×720.

**Recommendation:**
- Stay on GPU buffers when the aspect ratio matches (within ~1%); that's the common fullscreen toggle.
- Pad on the GPU for aspect changes later.
- This is half bug, half design, which is why it's here.

### 7.6 Audio: remember the toggles, pick the mic, survive a failure

`rec-15`

**Now:**
- The audio and mic toggles reset to the config every time.
- There's no mic device choice.
- A mic failure fails the whole start, or truncates the recording a second later with no explanation.

**Recommendation, in order:**
1. Remember the toggles in state.yaml.
2. If audio fails to start, record without it and toast why.
3. A mic picker (right-click on the mic toggle) later.

### 7.7 Encoder visibility

`rec-16`

**Now:**
- Only `daemon.log` says which encoder and GPU are in use.
- The README still describes VA-API or x264, with no mention of NVENC.
- When the compositor doesn't say which GPU it renders on, VA-API (the iGPU) still sorts first.

**Recommendation:**
- Add `encoder` and `gpu_frames` to `query status --json` and the pill tooltip, and add `screenie query encoders`.
- Find the GPU from the output's connector in sysfs when the protocol doesn't say.
- Update the README with the NVENC/VA packages for Arch and Debian.

### 7.8 Sharing clips

`rec-17`

**Now:** there's no trim and no size-targeted export, and quality-based encoding makes large files.

**Recommendation:**
- An "Export smaller…" card action that re-encodes in the background to 10, 25 or 50 MB (bitrate from duration).
- Then a minimal trim, per the North Star.
- Don't cap the bitrate globally.

### 7.9 Window recording silently becomes an area recording

`rec-18`

**Now:** where window capture isn't available, a window pick records the window's rectangle of the screen, with no hint.

**Recommendation:**
- Say so in the selector's window highlight or tooltip on those compositors.
- Toast only when window capture should have worked but the window match failed.

---

## 8. CLI, config and bars

### 8.1 Config errors and typos

`cli-ux-2`

**Now:**
- One bad value makes the running daemon keep the old config (only a log line says so).
- After the next restart, for example an auto-upgrade, the whole file silently falls back to defaults.
- Misspelled keys are silently ignored.

**Recommendation:**
- Warn about unknown keys with a "did you mean" suggestion (`serde_ignored` + `strsim`).
- Keep the last good config across restarts.
- Surface both through the toast (1.1) and a `screenie config check`.

### 8.2 `screenie config`

`cli-ux-3`, `cli-ux-4`

**Now:**
- There's no CLI way to find or check the config.
- `settings` and `pin` show in `--help` but aren't implemented.

**Recommendation:**
- Add `screenie config path | edit | check`; `edit` runs `$EDITOR`, then `check`.
- Hide `settings` and `pin` until they exist.
- Before the settings window writes to the config, make it patch the YAML in place so your comments survive, or at least write only non-default keys.

### 8.3 `--delay` gives no sign it's counting

`shot-ux-6`, `cli-ux-6`

**Now:** a silent timer. It can't be cancelled, and status says `selecting` throughout.

**Recommendation:**
- Reuse the recording countdown bubble: passive, and hidden before the snapshot.
- Add a `countdown` status state with remaining seconds.
- Let `screenie cancel`, or repeating the command, cancel it.

### 8.4 `record` flags

`cli-ux-7`

**Now:**
- `--audio` and `--mic` can only turn things on.
- There's no `--countdown N`, and no per-call copy/preview flags for recordings.
- `--no-copy` is hidden from `shot --help`.

**Recommendation:**
- Give `record` the flags `--[no-]audio`, `--[no-]mic`, `--countdown N`, `--[no-]copy` and `--no-preview`; `--countdown 0` makes instant clip hotkeys possible.
- Show `--no-copy` in help.

### 8.5 A hung daemon hangs every command

`cli-ux-8`

**Now:** there are no IPC timeouts, and status is answered on the UI thread. If the daemon hangs, bars freeze and keypresses pile up blocked `screenie` processes.

**Recommendation:**
- Answer status from a snapshot, off the UI thread.
- A ~2 s timeout on status and ping, with "daemon not responding (pid N)".
- Add `screenie quit --force`.

### 8.6 Cold starts and logging

`shot-ux-12`, `cli-ux-10`

**Now:**
- The first keypress after login spawns the daemon before capturing, so the frozen frame lags the keypress.
- `SCREENIE_LOG` only applies to the daemon when it spawns.

**Recommendation:**
- Document `exec screenie daemon` in the README (or `exec-once` for Hyprland, or a systemd user unit).
- Add `screenie daemon --replace`.
- Document `SCREENIE_LOG=debug screenie daemon --replace`.

### 8.7 What bars get

`cli-ux-11`

**Now:**
- The countdown has no remaining seconds.
- While a recording is saving, its path is empty.
- With no daemon, `--json` prints `pid: 0` rather than a `running` flag.
- The swaybar and i3blocks snippets need fixes.

**Recommendation:**
- Add `countdown_secs` and `running`, and keep the recording path while it's saving.
- Fix the snippets (`interval=persist`, merging with other status output).

### 8.8 `shot screen` without compositor IPC

`cli-ux-12`

**Now:** with no compositor IPC (COSMIC, river, labwc), "the focused output" can't be known.

A probe for the output under the pointer is being added as a bug fix. If it isn't enough there, the fallback with two or more monitors would be opening the pick-a-screen selector for keybinding use.

### 8.9 Docs that disagree with the product

`cli-ux-13`

**Now:** a few passages are out of date:
- NORTH_STAR says TOML and "Release = captured, copied, saved".
- ISSUES says copy is on by default.
- The screenie-app README says Done leads to a preview card.

**Recommendation:**
- Fix them.
- Extend the README config test to assert the documented `after_capture` defaults against `Config::default()`.

---

## 9. Calls made while fixing bugs

The fix agents had to pick a behaviour in a few places. Each is live on `main` now; veto any of them.

### 9.1 Screenshots

- **PNG compression is now "fast".** A 4K encode takes about 35 ms instead of 0.3–0.6 s, so the card and clipboard come sooner. Files come out somewhat larger; the size difference on a real desktop wasn't measured.
- **The shortcut again while the selector is up:**
  - The same mode again (`shot area` twice) cancels it; both requests exit 1.
  - A different mode (`shot area`, then `shot window`) switches the open selector to it.
  - `shot screen` takes the frozen snapshot the selector already has, and closes it.
- **Hiding screenie from its own captures** makes its surfaces fully transparent rather than unmapping them, so there's no Hyprland fade and cards keep their state. The catch: a Hyprland layerrule that blurs `screenie-*` without `ignorezero` would still show blur for that frame.
- **The window-mode editor** is an ordinary window, so it isn't hidden from screenshots. The overlay editor is.
- **`-o`:** an explicit file is overwritten, as before. A directory (or a path ending in `/`) gets a new, uniquely named file, and a name with no extension gets `.png`.
- **While `--delay` counts down**, status says `idle`, not a countdown state (see 8.3).

### 9.2 Preview cards

- **An open overlay editor pauses the cards' timers.** Each card gets its full time back when the editor closes. The cards are also raised above the editor, so they stay visible and clickable during the edit, pencil hidden. Check that this doesn't cover the part of a full-screen capture you're editing.
- **One stack per monitor.** Cards stay on the output they were taken on. A capture on another monitor no longer clears them.
- **Too many cards for the screen:** the oldest go first (never a hovered or saving one). There's no "+N" pile.
- **The card's hover layout** is now three rows. The size caption moved to the top, between dismiss and edit, so it can't collide with the Copied/Saved pill.
- **Dropped, per your call:** upgrades waiting for cards with unsaved screenshots.

### 9.3 Editor

- **`shot --edit` now waits for the edit.** It prints the saved path. It exits 0 after Done, or if anything was copied or saved, and 1 if the editor closed any other way with nothing kept.
- **Done still closes at once**, without waiting for its copy or save. If that fails, it's logged and reported to a waiting `shot --edit`, but the editor doesn't come back.
- **Save As always writes PNG.** `bug` becomes `bug.png`, and `shot.jpg` becomes `shot.png` rather than being encoded as JPEG.
- **A crop you're still adjusting** counts as unsaved work, and applies when you copy, save or switch tools. Only Esc or Cancel drops it.
- **Held keys:** typing, arrows, Backspace/Delete, `[` `]`, and undo/redo repeat. Everything else acts once per press.

### 9.4 Recording

- **Region recordings prefer wlr-screencopy** over ext-image-copy-capture on every GPU, since it copies just the region. This is what fixed the NVIDIA squash while keeping zero-copy. On AMD it moves region recordings off ext plus GPU cropping, which also worked.
- **Finishing a recording** waits as long as the file keeps growing, and gives up only after 30 s with nothing written. Then the data is kept as `NAME.mp4.part` beside the output, and the error says where.
- **A "Starting…" state on the pill** shows while the encoder is chosen. Stop during it cancels, since nothing has been recorded yet.
- **The pill and ring leave a window that goes fullscreen** over the recorded output, and come back when it leaves. It's a once-a-second poll, so they can linger up to a second.
- **A killed daemon still loses a running recording** (logout, OOM). Options are in ISSUES.

### 9.5 CLI and daemon

- **Exit codes:** `cancel` exits 0 after discarding. `stop`, `pause` and `cancel` with no daemon exit 2, without starting one. `stop` during the countdown exits 1, since nothing was recorded.
- **Config directories** expand `~`, `$VAR` and `${VAR}`, and relative paths are relative to home. An unset variable falls back to the default folder, with a warning.
- **Config reload** compares file contents rather than the modification time.
- **A new daemon waits up to 3 s** for one that's quitting, then refuses to start a second.
- **`state.yaml`** keeps the last region and captures under a `last:` section.
- **`--watch`** shows a state it doesn't know (from a newer daemon) as `unknown`, and counts it as busy, so an older CLI doesn't replace a newer daemon mid-task. Any command whose reader has gone away exits 0 quietly.

### 9.6 Not fixed

- **The clipboard empties when an upgrade replaces the daemon** (ISSUES has options).
- **A card appearing under a game's locked pointer grabs the keyboard on the next click** (`input-2`). This belongs with the overlay keyboard redesign, together with the stuck-Tab bug from Deadlock.
