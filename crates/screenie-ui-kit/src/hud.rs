//! The "HUD" look shared by every floating surface (selector toolbar, preview cards,
//! recording pill): dark translucent glass, a hairline highlight, and a soft shadow, so it
//! reads on any wallpaper and next to any app.

use gpui::prelude::*;
use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, Div, ElementId, FontWeight, Hsla, Rems, SharedString, Stateful, Window, div,
    point, px, rgba,
};

use crate::assets::Icon;
use crate::scale::ui;
use crate::tip::Tip;

pub mod color {
    use gpui::{Hsla, rgba};

    pub fn panel() -> Hsla {
        rgba(0x1c1c1ee6).into()
    }
    pub fn panel_solid() -> Hsla {
        rgba(0x1c1c1eff).into()
    }
    pub fn hairline() -> Hsla {
        rgba(0xffffff1a).into()
    }
    pub fn text() -> Hsla {
        rgba(0xffffffeb).into()
    }
    pub fn text_dim() -> Hsla {
        rgba(0xffffff8c).into()
    }
    pub fn hover() -> Hsla {
        rgba(0xffffff1a).into()
    }
    pub fn pressed() -> Hsla {
        rgba(0xffffff29).into()
    }
    pub fn selected() -> Hsla {
        rgba(0xffffff2e).into()
    }
    pub fn separator() -> Hsla {
        rgba(0xffffff1f).into()
    }
    /// macOS system blue.
    pub fn accent() -> Hsla {
        rgba(0x0a84ffff).into()
    }
    pub fn accent_hover() -> Hsla {
        rgba(0x2891ffff).into()
    }
    /// macOS system red.
    pub fn record() -> Hsla {
        rgba(0xff453aff).into()
    }
    pub fn record_hover() -> Hsla {
        rgba(0xff5a50ff).into()
    }
    pub fn warning() -> Hsla {
        rgba(0xffd60aff).into()
    }
    pub fn scrim(alpha: f32) -> Hsla {
        Hsla { h: 0.0, s: 0.0, l: 0.0, a: alpha }
    }
}

/// Layered shadow for floating panels.
pub fn panel_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: rgba(0x00000066).into(),
            offset: point(px(0.), px(12.)),
            blur_radius: px(32.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: rgba(0x00000059).into(),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        },
    ]
}

/// A panel's corner radius, and the padding between its edge and the controls in it.
const PANEL_RADIUS: f32 = 14.0;
const PANEL_PADDING: f32 = 3.0;

/// Added to a concentric inner radius so the gap *looks* even: along a curve both
/// edges are antialiased, and their grey pixels eat into the gap, so an exactly
/// concentric corner reads tighter than the straight sides. A rounder inner corner
/// pulls back from the panel's, opening the gap across the corner by about 0.4px per
/// pixel of radius (as type designers overshoot round letters).
const OPTICAL_CORRECTION: f32 = 0.5;

/// The corner radius of a control sitting in a [`panel`]: the panel's radius less the gap
/// between the two edges (padding and the 1px border), so both corners share a centre,
/// plus [`OPTICAL_CORRECTION`].
pub fn inner_radius() -> Rems {
    ui(PANEL_RADIUS - PANEL_PADDING - 1.0 + OPTICAL_CORRECTION)
}

/// A floating HUD panel (flex row by default). It swallows the pointer, so clicks on
/// its gaps never reach whatever is drawn underneath (e.g. the selector canvas).
pub fn panel() -> Div {
    div()
        .occlude()
        .flex()
        .flex_row()
        .items_center()
        .gap_0p5()
        .p(ui(PANEL_PADDING))
        .rounded(ui(PANEL_RADIUS))
        .bg(color::panel())
        .border_1()
        .border_color(color::hairline())
        .shadow(panel_shadow())
        .text_color(color::text())
        .text_size(ui(13.))
}

/// A small rounded label, e.g. the size readout next to a selection.
pub fn pill(text: impl Into<SharedString>) -> Div {
    div()
        .px(ui(8.))
        .py(ui(3.))
        .rounded(ui(7.))
        .bg(rgba(0x1c1c1ee0))
        .border_1()
        .border_color(color::hairline())
        .shadow(vec![BoxShadow {
            color: rgba(0x0000004d).into(),
            offset: point(px(0.), px(1.)),
            blur_radius: px(4.),
            spread_radius: px(0.),
            inset: false,
        }])
        .text_color(color::text())
        .text_size(ui(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

pub fn separator() -> Div {
    div().w(px(1.)).h(ui(20.)).mx_1().bg(color::separator())
}

/// A keyboard key hint.
pub fn keycap(text: impl Into<SharedString>) -> Div {
    div()
        .px(ui(5.))
        .rounded(ui(4.))
        .bg(color::hover())
        .text_color(color::text_dim())
        .text_size(ui(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonStyle {
    /// Transparent until hovered.
    #[default]
    Plain,
    /// Filled blue: the primary action.
    Accent,
    /// Filled red: recording.
    Record,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A HUD button with an icon, a label, or both.
#[derive(IntoElement)]
pub struct HudButton {
    id: ElementId,
    icon: Option<Icon>,
    icon_color: Option<Hsla>,
    label: Option<SharedString>,
    tooltip: Option<Tip>,
    selected: bool,
    disabled: bool,
    style: ButtonStyle,
    on_click: Option<ClickHandler>,
}

impl HudButton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            icon_color: None,
            label: None,
            tooltip: None,
            selected: false,
            disabled: false,
            style: ButtonStyle::Plain,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_color(mut self, color: Hsla) -> Self {
        self.icon_color = Some(color);
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<Tip>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for HudButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon_only = self.label.is_none();
        let (bg, hover_bg, fg) = match self.style {
            ButtonStyle::Plain if self.selected => (Some(color::selected()), color::selected(), color::text()),
            ButtonStyle::Plain => (None, color::hover(), color::text()),
            ButtonStyle::Accent => (Some(color::accent()), color::accent_hover(), gpui::white()),
            ButtonStyle::Record => (Some(color::record()), color::record_hover(), gpui::white()),
        };
        let mut el: Stateful<Div> = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1p5()
            .h(ui(30.))
            .rounded(inner_radius())
            .text_color(fg)
            .text_size(ui(13.))
            .font_weight(if self.style == ButtonStyle::Plain { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD });
        el = if icon_only { el.w(ui(32.)) } else { el.px(ui(if self.style == ButtonStyle::Plain { 10. } else { 14. })) };
        if let Some(bg) = bg {
            el = el.bg(bg);
        }
        if self.disabled {
            el = el.opacity(0.4);
        } else {
            el = el.cursor_pointer().hover(move |s| s.bg(hover_bg)).active(|s| s.opacity(0.85));
            if let Some(f) = self.on_click {
                el = el.on_click(f);
            }
        }
        if let Some(icon) = self.icon {
            el = el.child(icon.element().text_color(self.icon_color.unwrap_or(fg)));
        }
        if let Some(label) = self.label {
            el = el.child(label);
        }
        if let Some(tip) = self.tooltip {
            el = el.tooltip(tip.builder());
        }
        el
    }
}

/// Convenience for building a list of children with a common type.
pub fn children(items: impl IntoIterator<Item = AnyElement>) -> Vec<AnyElement> {
    items.into_iter().collect()
}
