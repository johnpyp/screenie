//! Tooltips in the HUD look: what a control does, its shortcut as keycaps, and a short
//! note only where the control doesn't explain itself.
//!
//! ```ignore
//! HudButton::new("area").icon(Icon::Area).tooltip(Tip::new("Area").key("1").note("Drag a region, or click a window"))
//! div().id("fill").tooltip(Tip::new("Fill").key("F").builder())
//! ```

use gpui::prelude::*;
use gpui::{AnyView, App, BoxShadow, Context, FontWeight, SharedString, Window, div, point, px, rgba};

use crate::hud::{color, keycap};
use crate::scale::ui;

/// A tooltip's content. Cheap to clone: it's rebuilt each time the tooltip shows.
#[derive(Debug, Clone)]
pub struct Tip {
    rows: Vec<Row>,
    note: Option<SharedString>,
}

/// A label and its shortcuts (alternatives, each a `+`-joined chord).
#[derive(Debug, Clone)]
struct Row {
    label: SharedString,
    keys: Vec<SharedString>,
}

impl Tip {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { rows: vec![Row { label: title.into(), keys: Vec::new() }], note: None }
    }

    /// A shortcut for the last row, e.g. `"Ctrl+Shift+Z"`. More than one are alternatives.
    pub fn key(mut self, chord: impl Into<SharedString>) -> Self {
        self.rows.last_mut().expect("a tip has a title").keys.push(chord.into());
        self
    }

    /// A related action under the title, e.g. "Save as" under "Save". Follow with `key`.
    pub fn also(mut self, label: impl Into<SharedString>) -> Self {
        self.rows.push(Row { label: label.into(), keys: Vec::new() });
        self
    }

    /// One short line on what isn't obvious from the control.
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// For GPUI's `.tooltip(...)` on any interactive element.
    pub fn builder(self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |_, cx| cx.new(|_| self.clone()).into()
    }
}

impl From<&'static str> for Tip {
    fn from(title: &'static str) -> Self {
        Tip::new(title)
    }
}

impl From<String> for Tip {
    fn from(title: String) -> Self {
        Tip::new(title)
    }
}

impl From<SharedString> for Tip {
    fn from(title: SharedString) -> Self {
        Tip::new(title)
    }
}

impl Render for Tip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.iter().enumerate().map(|(i, row)| {
            let keys = row.keys.iter().enumerate().flat_map(|(n, chord)| {
                let or = (n > 0).then(|| div().text_color(color::text_dim()).child("or").into_any_element());
                let chord = div()
                    .flex()
                    .flex_row()
                    .gap(ui(2.))
                    .children(chord.split('+').map(|k| keycap(k.to_owned())));
                or.into_iter().chain([chord.into_any_element()])
            });
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(ui(6.))
                .child(
                    div()
                        .flex_1()
                        .pr(ui(4.))
                        .when(i == 0, |d| d.font_weight(FontWeight::MEDIUM))
                        .when(i > 0, |d| d.text_color(color::text_dim()))
                        .child(row.label.clone()),
                )
                .children(keys)
        });
        // The transparent margin keeps the tip clear of the pointer, whichever side of it
        // GPUI puts the tip.
        div().p(ui(8.)).child(
            div()
                .flex()
                .flex_col()
                .gap(ui(3.))
                .px(ui(8.))
                .py(ui(5.))
                .rounded(ui(8.))
                .bg(color::panel_solid())
                .border_1()
                .border_color(color::hairline())
                .shadow(vec![BoxShadow {
                    color: rgba(0x00000066).into(),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .font_family(crate::FONT)
                .text_color(color::text())
                .text_size(ui(12.))
                .whitespace_nowrap()
                .children(rows)
                .children(self.note.clone().map(|note| div().text_color(color::text_dim()).child(note))),
        )
    }
}
