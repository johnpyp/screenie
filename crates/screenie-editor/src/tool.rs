//! The editor's tools.

use screenie_ui_kit::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Select, move and resize shapes.
    Select,
    Arrow,
    Line,
    Rectangle,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Step,
    Redact,
    Spotlight,
    Crop,
}

impl Tool {
    /// In toolbar order.
    pub const ALL: [Tool; 12] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Line,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Pen,
        Tool::Highlighter,
        Tool::Text,
        Tool::Step,
        Tool::Redact,
        Tool::Spotlight,
        Tool::Crop,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Text => "Text",
            Tool::Step => "Numbered step",
            Tool::Redact => "Pixelate / blur",
            Tool::Spotlight => "Spotlight",
            Tool::Crop => "Crop",
        }
    }

    /// What the tooltip adds to the name, where the tool has more to it than it shows.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Tool::Select => Some("Drag to move · Handles resize"),
            Tool::Arrow | Tool::Line => Some("Shift: snap to 15°"),
            Tool::Rectangle => Some("Shift: square · F: filled"),
            Tool::Ellipse => Some("Shift: circle · F: filled"),
            Tool::Highlighter => Some("Shift: straight"),
            Tool::Text => Some("F: on a label"),
            Tool::Step => Some("Click to place the next number"),
            Tool::Redact => Some("B again: pixelate or blur"),
            Tool::Spotlight => Some("Dims everything outside it"),
            Tool::Pen | Tool::Crop => None,
        }
    }

    /// The single-key shortcut.
    pub fn key(self) -> char {
        match self {
            Tool::Select => 'v',
            Tool::Arrow => 'a',
            Tool::Line => 'l',
            Tool::Rectangle => 'r',
            Tool::Ellipse => 'o',
            Tool::Pen => 'p',
            Tool::Highlighter => 'h',
            Tool::Text => 't',
            Tool::Step => 'n',
            Tool::Redact => 'b',
            Tool::Spotlight => 's',
            Tool::Crop => 'c',
        }
    }

    pub fn from_key(key: &str) -> Option<Tool> {
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else { return None };
        Tool::ALL.into_iter().find(|t| t.key() == c)
    }

    pub fn icon(self) -> Icon {
        match self {
            Tool::Select => Icon::Select,
            Tool::Arrow => Icon::Arrow,
            Tool::Line => Icon::Line,
            Tool::Rectangle => Icon::Rectangle,
            Tool::Ellipse => Icon::Ellipse,
            Tool::Pen => Icon::Pen,
            Tool::Highlighter => Icon::Highlighter,
            Tool::Text => Icon::Text,
            Tool::Step => Icon::Step,
            Tool::Redact => Icon::Pixelate,
            Tool::Spotlight => Icon::Spotlight,
            Tool::Crop => Icon::Crop,
        }
    }

    /// Whether the tool uses a colour.
    pub fn uses_color(self) -> bool {
        !matches!(self, Tool::Select | Tool::Redact | Tool::Spotlight | Tool::Crop)
    }

    /// Whether the size control means anything for the tool.
    pub fn uses_size(self) -> bool {
        !matches!(self, Tool::Select | Tool::Spotlight | Tool::Crop)
    }

    /// Whether the fill toggle applies.
    pub fn uses_fill(self) -> bool {
        matches!(self, Tool::Rectangle | Tool::Ellipse | Tool::Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_round_trip() {
        for tool in Tool::ALL {
            assert_eq!(Tool::from_key(&tool.key().to_string()), Some(tool));
        }
        assert_eq!(Tool::from_key("aa"), None);
        assert_eq!(Tool::from_key("z"), None);
    }
}
