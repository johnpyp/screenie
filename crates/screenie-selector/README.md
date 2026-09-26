# screenie-selector

The capture overlay: pick a region, window or screen.

```rust
let choice = screenie_selector::select(cx, Backdrop::Frozen(snapshot), config).await;
```

It opens one layer-shell surface per output, showing either a frozen `Snapshot`
(screenshots, with loupe) or the live desktop (recordings). It returns `None` on cancel.

`select_steered` takes the `Steering` half of a `Remote::new()`: the `Remote` cancels
the open selector or switches its mode from outside (the capture shortcut pressed
again). Its surfaces are kept out of captures taken while it's open
(`screenie_ui_kit::conceal`).

- **`model`**: the interaction state machine. It has no GUI types and is unit-tested:
  drawing, modifiers (square, from-center, move), handles, window/screen picking,
  keyboard, physical-pixel snapping, and one-output clamping for recordings.
- **`view`**: GPUI rendering of the model (dim, selection edge, handles, hover frames,
  size pill, loupe, toolbar) and event forwarding.

```sh
cargo run -p screenie-selector --example select -- [record] [adjust]
```
