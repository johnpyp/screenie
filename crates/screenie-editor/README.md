# screenie-editor

The annotation editor: an overlay over the capture's screen with the capture in place
(`Mode::Overlay`), or a regular window (`Mode::Window`).

```rust
screenie_editor::open(&image, EditorOptions { mode, output, placement, scale, style, .. }, |out, cx| {
    match out {
        Output::Copy(image) | Output::Save(image) | Output::SaveAs(image, _) => { /* … */ }
        Output::Done { image, copied, saved } => { /* skip what's already done */ }
        Output::Closed { style } => { /* remember for next time */ }
    }
    Ok(Some("Saved".into())) // optional toast
}, cx)?;

// Overlays are one at a time: refuse another while one is open (it shows the message).
if screenie_editor::overlay_busy("Finish this one first", cx) { /* … */ }
```

- **`Session`** is the interaction model, with no GPUI in it: the current tool and
  style, the selection, drag gestures (draw, move, handle), text editing with a caret,
  crop mode, and the Esc cascade. The view feeds it pointer events in image
  coordinates and `Key`s, and it drives a `screenie_annotate::Document`. It's covered
  by unit tests.
- **`Tool`** lists the tools with their names, keys, icons, and which style controls
  apply to each.
- **`raster`** keeps redraws cheap. It keeps a composite of every settled shape and
  re-renders only the live shape into a small tile, handing GPUI images and evicting
  the replaced ones from the atlas.
- **`view::Editor`** is the GPUI view. It has the canvas: in place as an overlay,
  otherwise fitted and never beyond logical 1:1. It also has the selection and crop
  chrome, the bars (hanging off the capture in an overlay, along the edges in a
  window), Ctrl+scroll sizing, the toast, and the "save your changes?" prompt. For Save
  As, an overlay closes and reopens from a clone of the session so the portal dialog
  isn't hidden underneath it.

What happens to the result (clipboard, files) is the caller's job; see
`screenie-app`'s `editor` module.
