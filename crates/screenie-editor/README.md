# screenie-editor

The annotation editor window.

```rust
screenie_editor::open(&image, EditorOptions { title, path, scale, palette, style, output }, |out, cx| {
    match out {
        Output::Copy(image) | Output::Save(image) | Output::SaveAs(image, _) | Output::Done(image) => { /* … */ }
        Output::Closed { style } => { /* remember for next time */ }
    }
    Ok(Some("Saved".into())) // optional toast
}, cx)?;
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
- **`view::Editor`** is the GPUI window: the canvas (fit, never beyond logical 1:1),
  selection and crop chrome, the tool, style and action bars, the toast, and the
  "save your changes?" prompt.

What happens to the result (clipboard, files, preview cards) is the caller's job; see
`screenie-app`'s `editor` module.
