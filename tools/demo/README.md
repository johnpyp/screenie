# Demo desktop

The README's screenshots and feature video come from here: a headless sway desktop
dressed up for the camera, a scripted tour of screenie on it, and a renderer that turns
the raw recording into the finished video.

```sh
cargo build --release -p screenie -p wlinput
tools/demo/desktop.sh start        # the desktop (4K at 2x, Catppuccin Mocha, waybar)
tools/demo/run.py video            # record the tour: .cache/demo/raw.mkv + timeline.json
tools/demo/render.py               # → .cache/demo/screenie-demo.mp4
tools/demo/desktop.sh stop && tools/demo/desktop.sh start
tools/demo/run.py stills           # the same tour, pausing for screenshots
tools/demo/stills.py               # → docs/media/*.webp
```

Restart the desktop between runs: the tour expects no preview cards on screen. Give it
~20 s after starting so btop's graph fills.

- **`desktop.sh`**: the session. Runs sway with its own home under `.cache/demo/home`
  (copied from `home/` on every start), so none of your config is read. Downloads the
  Bibata cursor and JetBrains Mono Nerd Font on first use; the wallpaper comes from
  `wallpaper.py`.
- **`run.py`**: the tour, as scenes. Moves the pointer along eased, slightly curved
  paths through one `wlinput -`, and logs a timeline (captions, key presses, where the
  camera should look). Window positions come from sway, preview card positions from
  screenie's card layout.
- **`render.py`**: syncs the timeline to the recording (the pointer jumps at its start),
  then per frame: repairs hitches (frames the capture missed become crossfades), moves
  an eased camera, and draws captions, keycaps, title and outro. Works at 4K and
  box-filters to 1080p, so text stays crisp while zoomed.
- **`load.py`**: a varying CPU load, so btop has something to draw.

It records with x264 on the CPU: wf-recorder's VA-API path delivers only about a third
of the frames at 4K.

Headless sway draws the pointer into the screen's image (there's no cursor plane), so a
frozen screen includes it. The tour presses PrtSc with the pointer where the selector's
toolbar will cover it.
