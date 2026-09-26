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
  `drag_handle`, `translate`, and bounds including stroke and shadow. Coordinates are
  image pixels, and `Style::size` is logical pixels scaled by the document's scale.
- **`Document`** records whole-state undo snapshots. `checkpoint` and
  `discard_checkpoint_if_unchanged` make a drag one step, and a click that changed
  nothing no step. `is_modified` compares against the last `mark_saved`.
- **`render`** draws into any pixmap covering part of the image (`render`,
  `draw_shapes`, `draw_base`). The editor uses that to re-render only a small tile
  around the shape being changed. Order: redactions, spotlights (one dimmed layer),
  then the rest.
- **`effects`**: pixelate, a three-box-blur Gaussian, and a mask blur for soft shadows.
- **`text`**: cosmic-text layout (caret positions, hit testing) and rasterization in
  Inter Bold with system-font fallback. Call `text::warm_up()` early; loading system
  fonts takes a moment.

`cargo run -p screenie-annotate --example sample` renders every shape kind to
`target/annotate-sample.png`, for eyeballing changes.
