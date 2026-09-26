# screenie-annotate

Annotation documents: a captured image, the shapes drawn on it, a crop, and undo
history, plus the tiny-skia renderer that draws them. It has no UI dependencies. The
editor draws its canvas with this crate, and export uses the same code, so the saved
file always matches what was on screen.

```rust
let mut doc = Document::new(&image, 2.0); // image pixels per logical pixel
let arrow = doc.make(Kind::Arrow { from, to }, Style::default());
doc.add(arrow); // one undo step
doc.undo();
let png_ready: Image = doc.export(); // shapes drawn, crop applied
```

- **`Shape`** (`Kind` + `Style`) covers geometry and editing: hit testing, handles,
  `drag_handle`, `translate`, and bounds including stroke and shadow. `touches(area)`
  says cheaply whether drawing it may change a pixel in an area, so a redraw skips the
  parts of a long diagonal arrow's bounds it doesn't cross. Coordinates are image
  pixels, and `Style::size` is logical pixels scaled by the document's scale.
- **`Document`** records whole-state undo snapshots. `checkpoint` and
  `discard_checkpoint_if_unchanged` make a drag one step, and a click that changed
  nothing no step. `is_modified` compares against the last `mark_saved`.
- **`render`**: `render_area` draws any part of the image the way the export does
  (redactions read beyond it as they need to, shadows come in from just outside it),
  so tiles put together match the export, up to tiny-skia antialiasing a path a hair
  differently where a tile cuts it off. `draw_over` draws shapes over a tile that
  already has what's under them. Order: redactions, the spotlight dim (on whenever the
  document has a spotlight, with holes for the ones drawn), then the rest.
- **`effects`**: pixelate (blocks aligned to the shape, wherever the tile is), a
  three-box-blur Gaussian, and soft shadows blurred only near what casts them, so a
  thin shape across a large area costs about what a small one does.
- **`text`**: cosmic-text layout (caret positions, hit testing) and rasterization in
  Inter Bold with system-font fallback. Call `text::warm_up()` early; loading system
  fonts takes a moment.

`cargo run -p screenie-annotate --example sample` renders every shape kind to
`.cache/annotate-sample.png`, for eyeballing changes. `cargo run --release -p
screenie-annotate --example bench` times what the editor does per pointer move on a 4K
capture.
