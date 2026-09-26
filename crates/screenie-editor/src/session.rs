//! The editor's interaction model, free of UI: tools, gestures, text editing and crop,
//! driving a [`Document`]. The view translates pointer and key events into calls here,
//! in image-pixel coordinates.

use screenie_annotate::{
    Document, Handle, Kind, Redaction, Shape, ShapeId, State, Style, box_from_drag, resize_box, snap_angle,
};
use screenie_core::{Point, Rect};

use crate::tool::Tool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Pointer precision in image pixels (it depends on the zoom).
#[derive(Debug, Clone, Copy)]
pub struct Reach {
    /// How close to a stroke counts as on it.
    pub tolerance: f64,
    /// How close to a handle counts as grabbing it.
    pub handle: f64,
}

/// What the pointer would do here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Crosshair,
    Text,
    Move,
    Grabbing,
    Resize(Handle),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Typed text (a character, or pasted text).
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Redraw,
    /// Finish: copy/save the result and close.
    Done,
    /// Close (asking first if there are unsaved changes).
    Close,
}

#[derive(Debug, Clone, Copy)]
enum Gesture {
    /// Drawing a new shape from `anchor`.
    Draw { id: ShapeId, anchor: Point },
    Move { id: ShapeId, last: Point },
    Handle { id: ShapeId, handle: Handle },
    CropDraw { anchor: Point },
    CropMove { last: Point },
    CropHandle { handle: Handle },
}

/// Text being typed into a text shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEdit {
    pub id: ShapeId,
    /// Byte offset of the caret.
    pub caret: usize,
}

#[derive(Debug, Clone, Copy)]
struct CropEdit {
    rect: Rect,
    previous_tool: Tool,
}

/// Smallest crop, in image pixels.
const MIN_CROP: f64 = 8.0;

#[derive(Clone)]
pub struct Session {
    doc: Document,
    tool: Tool,
    style: Style,
    redaction: Redaction,
    selected: Option<ShapeId>,
    gesture: Option<Gesture>,
    text: Option<TextEdit>,
    crop: Option<CropEdit>,
    /// The document as opened, and as last copied and saved: to skip copying or saving
    /// again what already was, and to know when closing would lose work.
    pristine: State,
    copied: Option<State>,
    saved: Option<State>,
}

impl Session {
    /// `on_disk`: the image is already saved as it is (opened from a file).
    pub fn new(doc: Document, style: Style, on_disk: bool) -> Self {
        let pristine = doc.state().clone();
        Self {
            saved: on_disk.then(|| pristine.clone()),
            copied: None,
            pristine,
            doc,
            tool: Tool::Arrow,
            style,
            redaction: Redaction::default(),
            selected: None,
            gesture: None,
            text: None,
            crop: None,
        }
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn redaction(&self) -> Redaction {
        self.redaction
    }

    pub fn selected(&self) -> Option<&Shape> {
        self.selected.and_then(|id| self.doc.shape(id))
    }

    pub fn text_edit(&self) -> Option<TextEdit> {
        self.text
    }

    /// The crop rectangle being adjusted, in crop mode.
    pub fn crop_edit(&self) -> Option<Rect> {
        self.crop.map(|c| c.rect)
    }

    /// Whether a pointer gesture is in progress.
    pub fn is_dragging(&self) -> bool {
        self.gesture.is_some()
    }

    /// Whether the selection was just drawn and is still being dragged out (its handles
    /// would only get in the way).
    pub fn is_drawing(&self) -> bool {
        matches!(self.gesture, Some(Gesture::Draw { .. }))
    }

    /// The shape changing under the user's hands, which the view draws separately from
    /// the rest so only it needs re-rendering.
    pub fn live(&self) -> Option<ShapeId> {
        match self.gesture {
            Some(Gesture::Draw { id, .. } | Gesture::Move { id, .. } | Gesture::Handle { id, .. }) => Some(id),
            _ => self.text.map(|t| t.id),
        }
    }

    // Tools and style.

    pub fn set_tool(&mut self, tool: Tool) {
        if tool == self.tool && tool != Tool::Redact {
            return;
        }
        self.commit_text();
        self.gesture = None;
        match tool {
            Tool::Crop => self.enter_crop(),
            _ => {
                self.crop = None;
                // Pressing the redaction key again switches between pixelate and blur.
                if tool == Tool::Redact && self.tool == Tool::Redact {
                    let next = match self.redaction {
                        Redaction::Pixelate => Redaction::Blur,
                        Redaction::Blur => Redaction::Pixelate,
                    };
                    self.set_redaction(next);
                }
                self.tool = tool;
            }
        }
    }

    pub fn set_color(&mut self, color: screenie_annotate::Color) {
        self.restyle(Style { color, ..self.style });
    }

    pub fn set_size(&mut self, size: f32) {
        self.restyle(Style { size, ..self.style });
    }

    /// Step through the size presets.
    pub fn step_size(&mut self, steps: i32) {
        self.set_size(Style::step_size(self.style.size, steps));
    }

    /// Step the size from the scroll wheel: a run of these on one shape (keeping its
    /// geometry) is a single undo step.
    pub fn scroll_size(&mut self, steps: i32) -> bool {
        let size = Style::step_size(self.style.size, steps);
        if size == self.style.size {
            return false;
        }
        self.style.size = size;
        if let Some(id) = self.target() {
            self.doc.restyle_merging(id, self.style);
        }
        true
    }

    pub fn toggle_fill(&mut self) {
        self.restyle(Style { fill: !self.style.fill, ..self.style });
    }

    pub fn set_redaction(&mut self, mode: Redaction) {
        self.redaction = mode;
        if let Some(id) = self.target()
            && let Some(Kind::Redact { mode: current, .. }) = self.doc.shape(id).map(|s| &s.kind)
            && *current != mode
        {
            self.doc.checkpoint();
            if let Some(Shape { kind: Kind::Redact { mode: m, .. }, .. }) = self.doc.shape_mut(id) {
                *m = mode;
            }
        }
    }

    /// Change the style for new shapes and for the shape being edited.
    fn restyle(&mut self, style: Style) {
        self.style = style;
        if let Some(id) = self.target() {
            self.doc.restyle(id, style);
        }
    }

    /// The shape style changes apply to.
    fn target(&self) -> Option<ShapeId> {
        self.text.map(|t| t.id).or(self.selected)
    }

    fn select(&mut self, id: Option<ShapeId>) {
        self.selected = id;
        if let Some(shape) = id.and_then(|id| self.doc.shape(id)) {
            self.style = shape.style;
            if let Kind::Redact { mode, .. } = shape.kind {
                self.redaction = mode;
            }
        }
    }

    // History.

    pub fn undo(&mut self) -> bool {
        self.commit_text();
        self.gesture = None;
        let changed = self.doc.undo();
        self.forget_missing();
        changed
    }

    pub fn redo(&mut self) -> bool {
        self.commit_text();
        self.gesture = None;
        let changed = self.doc.redo();
        self.forget_missing();
        changed
    }

    /// After undo/redo: drop a selection that no longer exists, and show the style the
    /// selection has now.
    fn forget_missing(&mut self) {
        self.select(self.selected.filter(|id| self.doc.shape(*id).is_some()));
    }

    pub fn delete_selected(&mut self) -> bool {
        let Some(id) = self.selected.take() else { return false };
        self.doc.remove(id).is_some()
    }

    pub fn duplicate_selected(&mut self) -> bool {
        let Some(shape) = self.selected().cloned() else { return false };
        let mut copy = self.doc.make(shape.kind, shape.style);
        let offset = 12.0 * self.doc.scale() as f64;
        copy.translate(offset, offset);
        let id = copy.id;
        self.doc.add(copy);
        self.selected = Some(id);
        true
    }

    /// The current annotations are on the clipboard.
    pub fn mark_copied(&mut self) {
        self.commit_text();
        self.copied = Some(self.doc.state().clone());
    }

    /// The current annotations are saved.
    pub fn mark_saved(&mut self) {
        self.commit_text();
        self.saved = Some(self.doc.state().clone());
    }

    pub fn is_copied(&self) -> bool {
        self.copied.as_ref() == Some(self.doc.state())
    }

    pub fn is_saved(&self) -> bool {
        self.saved.as_ref() == Some(self.doc.state())
    }

    /// Whether the image was ever saved (opened from a file, or saved since).
    pub fn has_file(&self) -> bool {
        self.saved.is_some()
    }

    /// Whether closing now would lose annotations: there are some, and they were
    /// neither copied nor saved. (Undoing back to a delivered state counts as safe.)
    pub fn has_unsaved_work(&self) -> bool {
        *self.doc.state() != self.pristine && !self.is_copied() && !self.is_saved()
    }

    /// The finished image. Commits any text being typed first.
    /// The finished image. Handing it out (copy, save) also settles the editor: typing
    /// is committed and the selection dropped, so one Esc afterwards closes.
    pub fn export(&mut self) -> screenie_core::Image {
        self.settle();
        self.doc.export()
    }

    /// Commit any text being typed and deselect.
    pub fn settle(&mut self) {
        self.commit_text();
        self.selected = None;
    }

    // Pointer.

    pub fn press(&mut self, p: Point, reach: Reach, clicks: usize) -> bool {
        if self.crop.is_some() {
            self.crop_press(p, reach);
            return true;
        }
        if let Some(edit) = self.text {
            if self.doc.shape(edit.id).is_some_and(|s| s.hit(p, reach.tolerance, self.doc.scale())) {
                self.text = Some(TextEdit { caret: self.caret_at(edit.id, p), ..edit });
                return true;
            }
            self.commit_text();
            if self.tool != Tool::Text {
                return true;
            }
        }
        let hit = self.doc.hit(p, reach.tolerance);
        let hit_kind = hit.and_then(|id| self.doc.shape(id)).map(|s| s.kind.clone());

        if clicks >= 2 && matches!(hit_kind, Some(Kind::Text { .. })) {
            self.edit_text(hit.expect("hit"), p);
            return true;
        }
        if let Some(handle) = self.handle_at(p, reach) {
            let id = self.selected.expect("handles belong to the selection");
            self.doc.checkpoint();
            self.gesture = Some(Gesture::Handle { id, handle });
            return true;
        }

        match self.tool {
            Tool::Select => match hit {
                Some(id) => self.start_move(id, p),
                None => self.select(None),
            },
            Tool::Text => match (hit, hit_kind) {
                (Some(id), Some(Kind::Text { .. })) => self.edit_text(id, p),
                _ => self.new_text(p),
            },
            Tool::Step => match (hit, hit_kind) {
                (Some(id), Some(Kind::Step { .. })) => self.start_move(id, p),
                _ => {
                    let shape = self.doc.make(Kind::Step { center: p }, self.style);
                    let id = shape.id;
                    self.doc.add(shape);
                    self.selected = None;
                    self.gesture = Some(Gesture::Move { id, last: p });
                }
            },
            Tool::Crop => {}
            tool => {
                match hit {
                    Some(id) if hit == self.selected => self.start_move(id, p),
                    _ => self.start_drawing(tool, p),
                }
            }
        }
        true
    }

    pub fn drag(&mut self, p: Point, mods: Modifiers) -> bool {
        let Some(gesture) = self.gesture else { return false };
        match gesture {
            Gesture::Draw { id, anchor } => {
                let Some(shape) = self.doc.shape_mut(id) else { return false };
                match &mut shape.kind {
                    Kind::Arrow { to, .. } | Kind::Line { to, .. } => {
                        *to = if mods.shift { snap_angle(anchor, p) } else { p };
                    }
                    Kind::Rectangle { rect } | Kind::Ellipse { rect } | Kind::Redact { rect, .. } | Kind::Spotlight { rect } => {
                        *rect = box_from_drag(anchor, p, mods.shift);
                    }
                    Kind::Pen { points } => {
                        if points.last().is_none_or(|last| last.distance(p) >= 1.0) {
                            points.push(p);
                        }
                    }
                    Kind::Highlighter { points } => {
                        if mods.shift {
                            *points = vec![anchor, snap_angle(anchor, p)];
                        } else if points.last().is_none_or(|last| last.distance(p) >= 1.0) {
                            points.push(p);
                        }
                    }
                    _ => {}
                }
            }
            Gesture::Move { id, last } => {
                if let Some(shape) = self.doc.shape_mut(id) {
                    shape.translate(p.x - last.x, p.y - last.y);
                }
                self.gesture = Some(Gesture::Move { id, last: p });
            }
            Gesture::Handle { id, handle } => {
                if let Some(shape) = self.doc.shape_mut(id) {
                    shape.drag_handle(handle, p, mods.shift);
                }
            }
            Gesture::CropDraw { anchor } => self.set_crop_rect(box_from_drag(anchor, p, mods.shift)),
            Gesture::CropMove { last } => {
                if let Some(crop) = &mut self.crop {
                    let bounds = self.doc.bounds();
                    crop.rect = crop.rect.translate(p.x - last.x, p.y - last.y).clamp_within(&bounds);
                }
                self.gesture = Some(Gesture::CropMove { last: p });
            }
            Gesture::CropHandle { handle } => {
                if let Some(crop) = self.crop {
                    self.set_crop_rect(resize_box(crop.rect, handle, p, mods.shift));
                }
            }
        }
        true
    }

    pub fn release(&mut self) -> bool {
        let Some(gesture) = self.gesture.take() else { return false };
        match gesture {
            Gesture::Draw { id, .. } => {
                let degenerate = self.doc.shape(id).is_none_or(|s| s.is_degenerate());
                if degenerate {
                    // A click with a drawing tool draws nothing.
                    self.doc.state_mut().shapes.retain(|s| s.id != id);
                    self.doc.discard_checkpoint_if_unchanged();
                    self.selected = None;
                } else if !matches!(self.tool, Tool::Pen | Tool::Highlighter) {
                    self.selected = Some(id);
                }
            }
            Gesture::Move { .. } | Gesture::Handle { .. } => self.doc.discard_checkpoint_if_unchanged(),
            Gesture::CropDraw { .. } | Gesture::CropMove { .. } | Gesture::CropHandle { .. } => {}
        }
        true
    }

    /// The cursor for a pointer resting at `p`.
    pub fn hover(&self, p: Point, reach: Reach) -> Cursor {
        if let Some(gesture) = self.gesture {
            return match gesture {
                Gesture::Move { .. } | Gesture::CropMove { .. } => Cursor::Grabbing,
                Gesture::Handle { handle, .. } | Gesture::CropHandle { handle } => Cursor::Resize(handle),
                Gesture::Draw { .. } | Gesture::CropDraw { .. } => Cursor::Crosshair,
            };
        }
        if let Some(crop) = self.crop {
            if let Some(handle) = crop_handle_at(crop.rect, p, reach) {
                return Cursor::Resize(handle);
            }
            return if crop.rect.contains(p) { Cursor::Move } else { Cursor::Crosshair };
        }
        if let Some(handle) = self.handle_at(p, reach) {
            return Cursor::Resize(handle);
        }
        let hit = self.doc.hit(p, reach.tolerance);
        if let Some(edit) = self.text
            && hit == Some(edit.id)
        {
            return Cursor::Text;
        }
        match self.tool {
            Tool::Select if hit.is_some() => Cursor::Move,
            Tool::Select => Cursor::Arrow,
            Tool::Text => Cursor::Text,
            Tool::Step if hit.is_some_and(|id| matches!(self.doc.shape(id).map(|s| &s.kind), Some(Kind::Step { .. }))) => {
                Cursor::Move
            }
            _ if hit.is_some() && hit == self.selected => Cursor::Move,
            _ => Cursor::Crosshair,
        }
    }

    fn handle_at(&self, p: Point, reach: Reach) -> Option<Handle> {
        let shape = self.selected()?;
        shape.handles().into_iter().filter(|(_, at)| at.distance(p) <= reach.handle).min_by(|a, b| {
            a.1.distance(p).total_cmp(&b.1.distance(p))
        }).map(|(h, _)| h)
    }

    fn start_move(&mut self, id: ShapeId, p: Point) {
        self.select(Some(id));
        self.doc.checkpoint();
        self.gesture = Some(Gesture::Move { id, last: p });
    }

    fn start_drawing(&mut self, tool: Tool, p: Point) {
        let rect = Rect::new(p.x, p.y, 0.0, 0.0);
        let kind = match tool {
            Tool::Arrow => Kind::Arrow { from: p, to: p },
            Tool::Line => Kind::Line { from: p, to: p },
            Tool::Rectangle => Kind::Rectangle { rect },
            Tool::Ellipse => Kind::Ellipse { rect },
            Tool::Pen => Kind::Pen { points: vec![p] },
            Tool::Highlighter => Kind::Highlighter { points: vec![p] },
            Tool::Redact => Kind::Redact { rect, mode: self.redaction },
            Tool::Spotlight => Kind::Spotlight { rect },
            Tool::Select | Tool::Text | Tool::Step | Tool::Crop => return,
        };
        let shape = self.doc.make(kind, self.style);
        let id = shape.id;
        self.doc.add(shape);
        self.selected = None;
        self.gesture = Some(Gesture::Draw { id, anchor: p });
    }

    // Text.

    fn new_text(&mut self, p: Point) {
        let mut shape = self.doc.make(Kind::Text { origin: p, text: String::new() }, self.style);
        // Centre the first line on the click.
        let line = shape.text_block(self.doc.scale()).map_or(0.0, |b| b.line_height() as f64);
        shape.translate(0.0, -line / 2.0);
        let id = shape.id;
        self.doc.add(shape);
        self.selected = None;
        self.text = Some(TextEdit { id, caret: 0 });
    }

    fn edit_text(&mut self, id: ShapeId, p: Point) {
        self.select(Some(id));
        self.selected = None;
        self.doc.checkpoint();
        self.text = Some(TextEdit { id, caret: self.caret_at(id, p) });
    }

    fn caret_at(&self, id: ShapeId, p: Point) -> usize {
        let Some(shape) = self.doc.shape(id) else { return 0 };
        let (Kind::Text { origin, .. }, Some(block)) = (&shape.kind, shape.text_block(self.doc.scale())) else {
            return 0;
        };
        block.hit((p.x - origin.x) as f32, (p.y - origin.y) as f32)
    }

    /// Finish typing. Empty text disappears (and leaves no undo step).
    pub fn commit_text(&mut self) {
        let Some(edit) = self.text.take() else { return };
        if self.doc.shape(edit.id).is_some_and(|s| s.is_degenerate()) {
            self.doc.state_mut().shapes.retain(|s| s.id != edit.id);
        }
        self.doc.discard_checkpoint_if_unchanged();
    }

    fn edit_string(&mut self, f: impl FnOnce(&mut String, &mut usize)) {
        let Some(edit) = &mut self.text else { return };
        if let Some(Shape { kind: Kind::Text { text, .. }, .. }) = self.doc.shape_mut(edit.id) {
            f(text, &mut edit.caret);
        }
    }

    fn text_key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Escape => self.commit_text(),
            Key::Text(typed) => self.edit_string(|text, caret| {
                let typed: String = typed.chars().filter(|c| !c.is_control() || *c == '\n').collect();
                text.insert_str(*caret, &typed);
                *caret += typed.len();
            }),
            Key::Enter => self.edit_string(|text, caret| {
                text.insert(*caret, '\n');
                *caret += 1;
            }),
            Key::Backspace => self.edit_string(|text, caret| {
                if let Some((i, _)) = text[..*caret].char_indices().next_back() {
                    text.replace_range(i..*caret, "");
                    *caret = i;
                }
            }),
            Key::Delete => self.edit_string(|text, caret| {
                if let Some(c) = text[*caret..].chars().next() {
                    text.replace_range(*caret..*caret + c.len_utf8(), "");
                }
            }),
            Key::Left => self.edit_string(|text, caret| {
                *caret = text[..*caret].char_indices().next_back().map_or(0, |(i, _)| i);
            }),
            Key::Right => self.edit_string(|text, caret| {
                *caret += text[*caret..].chars().next().map_or(0, char::len_utf8);
            }),
            Key::Home => self.edit_string(|text, caret| *caret = text[..*caret].rfind('\n').map_or(0, |i| i + 1)),
            Key::End => self.edit_string(|text, caret| *caret += text[*caret..].find('\n').unwrap_or(text.len() - *caret)),
            Key::Up | Key::Down => return Outcome::Nothing,
        }
        Outcome::Redraw
    }

    // Keys.

    pub fn key(&mut self, key: Key, mods: Modifiers) -> Outcome {
        if self.text.is_some() {
            return self.text_key(key);
        }
        if self.crop.is_some() {
            return match key {
                Key::Escape => {
                    self.exit_crop();
                    Outcome::Redraw
                }
                Key::Enter => {
                    self.apply_crop();
                    Outcome::Redraw
                }
                _ => Outcome::Nothing,
            };
        }
        match key {
            Key::Escape if self.gesture.is_some() => Outcome::Nothing,
            Key::Escape if self.selected.is_some() => {
                self.selected = None;
                Outcome::Redraw
            }
            Key::Escape => Outcome::Close,
            Key::Enter => Outcome::Done,
            Key::Backspace | Key::Delete => redraw(self.delete_selected()),
            Key::Left | Key::Right | Key::Up | Key::Down => {
                let step = self.doc.scale() as f64 * if mods.shift { 10.0 } else { 1.0 };
                let (dx, dy) = match key {
                    Key::Left => (-step, 0.0),
                    Key::Right => (step, 0.0),
                    Key::Up => (0.0, -step),
                    _ => (0.0, step),
                };
                let Some(id) = self.selected else { return Outcome::Nothing };
                self.doc.checkpoint();
                if let Some(shape) = self.doc.shape_mut(id) {
                    shape.translate(dx, dy);
                }
                Outcome::Redraw
            }
            Key::Text(t) => match t.as_str() {
                "[" => {
                    self.step_size(-1);
                    Outcome::Redraw
                }
                "]" => {
                    self.step_size(1);
                    Outcome::Redraw
                }
                "f" => {
                    self.toggle_fill();
                    Outcome::Redraw
                }
                // 1–9 and 0 pick the ten sizes.
                d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                    let i = (d.as_bytes()[0] - b'0') as usize;
                    self.set_size(Style::SIZES[(i + 9) % 10]);
                    Outcome::Redraw
                }
                other => match crate::tool::Tool::from_key(&other.to_lowercase()) {
                    Some(tool) => {
                        self.set_tool(tool);
                        Outcome::Redraw
                    }
                    None => Outcome::Nothing,
                },
            },
            Key::Home | Key::End => Outcome::Nothing,
        }
    }

    // Crop.

    fn enter_crop(&mut self) {
        self.selected = None;
        let rect = self.doc.crop().unwrap_or_else(|| self.doc.bounds());
        self.crop = Some(CropEdit { rect, previous_tool: self.tool });
        self.tool = Tool::Crop;
    }

    fn exit_crop(&mut self) {
        if let Some(crop) = self.crop.take() {
            self.tool = crop.previous_tool;
        }
        self.gesture = None;
    }

    /// Keep the adjusted crop and leave crop mode.
    pub fn apply_crop(&mut self) {
        if let Some(crop) = self.crop {
            self.doc.set_crop(Some(crop.rect));
        }
        self.exit_crop();
    }

    pub fn cancel_crop(&mut self) {
        self.exit_crop();
    }

    /// Undo the crop entirely.
    pub fn reset_crop(&mut self) {
        if let Some(crop) = &mut self.crop {
            crop.rect = self.doc.bounds();
        }
    }

    fn set_crop_rect(&mut self, rect: Rect) {
        let bounds = self.doc.bounds();
        if let (Some(crop), Some(rect)) = (&mut self.crop, rect.intersection(&bounds))
            && rect.width >= MIN_CROP
            && rect.height >= MIN_CROP
        {
            crop.rect = rect;
        }
    }

    fn crop_press(&mut self, p: Point, reach: Reach) {
        let Some(crop) = self.crop else { return };
        self.gesture = Some(if let Some(handle) = crop_handle_at(crop.rect, p, reach) {
            Gesture::CropHandle { handle }
        } else if crop.rect.contains(p) {
            Gesture::CropMove { last: p }
        } else {
            Gesture::CropDraw { anchor: p }
        });
    }
}

fn crop_handle_at(rect: Rect, p: Point, reach: Reach) -> Option<Handle> {
    Handle::BOX
        .into_iter()
        .map(|h| (h, h.position(&rect).distance(p)))
        .filter(|(_, d)| *d <= reach.handle)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

fn redraw(changed: bool) -> Outcome {
    if changed { Outcome::Redraw } else { Outcome::Nothing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenie_core::{Image, PixelFormat};

    const REACH: Reach = Reach { tolerance: 4.0, handle: 8.0 };
    const NONE: Modifiers = Modifiers { shift: false, ctrl: false, alt: false };

    fn session() -> Session {
        Session::new(Document::new(&Image::new(400, 300, PixelFormat::Rgbx), 1.0), Style::default(), false)
    }

    fn pt(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn drag(s: &mut Session, from: Point, to: Point) {
        s.press(from, REACH, 1);
        s.drag(pt((from.x + to.x) / 2.0, (from.y + to.y) / 2.0), NONE);
        s.drag(to, NONE);
        s.release();
    }

    fn typed(s: &mut Session, text: &str) {
        s.key(Key::Text(text.into()), NONE);
    }

    #[test]
    fn drawing_an_arrow_selects_it_and_undoes_in_one_step() {
        let mut s = session();
        drag(&mut s, pt(10.0, 10.0), pt(100.0, 80.0));
        assert_eq!(s.doc().shapes().len(), 1);
        assert!(matches!(s.selected().map(|s| &s.kind), Some(Kind::Arrow { .. })));
        assert!(s.undo());
        assert!(s.doc().shapes().is_empty());
        assert!(s.selected().is_none());
        assert!(!s.doc().can_undo());
    }

    #[test]
    fn a_click_with_a_drawing_tool_draws_nothing() {
        let mut s = session();
        s.set_tool(Tool::Rectangle);
        s.press(pt(50.0, 50.0), REACH, 1);
        s.release();
        assert!(s.doc().shapes().is_empty());
        assert!(!s.doc().can_undo());
    }

    #[test]
    fn handles_reshape_the_selection() {
        let mut s = session();
        s.set_tool(Tool::Rectangle);
        drag(&mut s, pt(10.0, 10.0), pt(110.0, 60.0));
        drag(&mut s, pt(110.0, 60.0), pt(150.0, 100.0));
        assert_eq!(s.doc().shapes().len(), 1);
        assert_eq!(s.selected().unwrap().bounds(1.0), Rect::new(10.0, 10.0, 140.0, 90.0));
        s.undo();
        assert_eq!(s.doc().shapes()[0].bounds(1.0), Rect::new(10.0, 10.0, 100.0, 50.0));
    }

    #[test]
    fn dragging_the_selected_shape_moves_it_but_others_are_drawn_over() {
        let mut s = session();
        s.set_tool(Tool::Rectangle);
        s.set_tool(Tool::Rectangle);
        drag(&mut s, pt(10.0, 10.0), pt(110.0, 60.0));
        // On the selected shape's edge: moves it.
        drag(&mut s, pt(35.0, 10.0), pt(45.0, 20.0));
        assert_eq!(s.doc().shapes().len(), 1);
        assert_eq!(s.doc().shapes()[0].bounds(1.0), Rect::new(20.0, 20.0, 100.0, 50.0));
        // Elsewhere: a new rectangle.
        drag(&mut s, pt(200.0, 200.0), pt(250.0, 250.0));
        assert_eq!(s.doc().shapes().len(), 2);
    }

    #[test]
    fn select_tool_picks_moves_and_deletes() {
        let mut s = session();
        drag(&mut s, pt(10.0, 150.0), pt(200.0, 150.0));
        s.set_tool(Tool::Select);
        s.key(Key::Escape, NONE);
        assert!(s.selected().is_none());
        s.press(pt(100.0, 151.0), REACH, 1);
        assert!(s.selected().is_some());
        s.release();
        assert!(!s.doc().can_redo());
        assert_eq!(s.key(Key::Delete, NONE), Outcome::Redraw);
        assert!(s.doc().shapes().is_empty());
    }

    #[test]
    fn text_is_typed_committed_and_empty_text_vanishes() {
        let mut s = session();
        s.set_tool(Tool::Text);
        s.press(pt(50.0, 50.0), REACH, 1);
        s.release();
        assert!(s.text_edit().is_some());
        typed(&mut s, "Hi");
        s.key(Key::Enter, NONE);
        typed(&mut s, "yo");
        s.key(Key::Backspace, NONE);
        s.key(Key::Left, NONE);
        typed(&mut s, "é");
        assert_eq!(s.key(Key::Escape, NONE), Outcome::Redraw);
        assert!(s.text_edit().is_none());
        match &s.doc().shapes()[0].kind {
            Kind::Text { text, .. } => assert_eq!(text, "Hi\néy"),
            other => panic!("{other:?}"),
        }
        // One undo removes the whole text.
        s.undo();
        assert!(s.doc().shapes().is_empty());

        // Clicking away from an empty box leaves nothing behind.
        s.press(pt(50.0, 50.0), REACH, 1);
        s.release();
        s.set_tool(Tool::Arrow);
        assert!(s.doc().shapes().is_empty());
        assert!(!s.doc().can_redo() || s.doc().shapes().is_empty());
    }

    #[test]
    fn keys_type_into_text_instead_of_switching_tools() {
        let mut s = session();
        s.set_tool(Tool::Text);
        s.press(pt(50.0, 50.0), REACH, 1);
        typed(&mut s, "a");
        assert_eq!(s.tool(), Tool::Text);
        s.key(Key::Escape, NONE);
        typed(&mut s, "a");
        assert_eq!(s.tool(), Tool::Arrow);
    }

    #[test]
    fn steps_stay_on_the_tool_and_number_themselves() {
        let mut s = session();
        s.set_tool(Tool::Step);
        for x in [50.0, 100.0, 150.0] {
            s.press(pt(x, 50.0), REACH, 1);
            s.release();
        }
        assert_eq!(s.doc().shapes().len(), 3);
        let last = s.doc().shapes()[2].id;
        assert_eq!(s.doc().step_number(last), 3);
        // Dragging an existing step moves it rather than adding one.
        drag(&mut s, pt(100.0, 50.0), pt(100.0, 120.0));
        assert_eq!(s.doc().shapes().len(), 3);
    }

    #[test]
    fn style_changes_apply_to_the_selection() {
        let mut s = session();
        drag(&mut s, pt(10.0, 10.0), pt(100.0, 80.0));
        s.set_size(8.0);
        assert_eq!(s.selected().unwrap().style.size, 8.0);
        s.key(Key::Escape, NONE);
        drag(&mut s, pt(10.0, 200.0), pt(100.0, 280.0));
        assert_eq!(s.selected().unwrap().style.size, 8.0);
        s.key(Key::Text("1".into()), NONE);
        assert_eq!(s.style().size, Style::SIZES[0]);
        s.key(Key::Text("0".into()), NONE);
        assert_eq!(s.style().size, Style::SIZES[9]);
    }

    #[test]
    fn exporting_deselects_so_one_escape_closes() {
        let mut s = session();
        drag(&mut s, pt(10.0, 10.0), pt(100.0, 80.0));
        assert!(s.selected().is_some());
        s.export();
        assert!(s.selected().is_none());
        assert_eq!(s.key(Key::Escape, NONE), Outcome::Close);
    }

    #[test]
    fn scrolling_resizes_the_selection_in_one_undo_step() {
        let mut s = session();
        drag(&mut s, pt(10.0, 10.0), pt(100.0, 80.0));
        let before = s.selected().unwrap().clone();
        for _ in 0..3 {
            assert!(s.scroll_size(1));
        }
        let after = s.selected().unwrap();
        assert_eq!(after.style.size, Style::step_size(before.style.size, 3));
        // Thicker, same geometry.
        assert_eq!(after.kind, before.kind);
        s.undo();
        assert_eq!(s.doc().shape(before.id).unwrap().style.size, before.style.size);
        assert_eq!(s.doc().shapes().len(), 1);
        // The style bar follows the selection back.
        assert_eq!(s.style().size, before.style.size);
        // At the end of the range nothing changes.
        s.set_size(Style::SIZES[9]);
        assert!(!s.scroll_size(1));
    }

    #[test]
    fn crop_mode_applies_on_enter_and_cancels_on_escape() {
        let mut s = session();
        s.set_tool(Tool::Rectangle);
        s.set_tool(Tool::Crop);
        assert_eq!(s.crop_edit(), Some(s.doc().bounds()));
        drag(&mut s, pt(0.0, 0.0), pt(50.0, 40.0));
        assert_eq!(s.crop_edit(), Some(Rect::new(50.0, 40.0, 350.0, 260.0)));
        s.key(Key::Enter, NONE);
        assert_eq!(s.doc().crop(), Some(Rect::new(50.0, 40.0, 350.0, 260.0)));
        assert_eq!(s.tool(), Tool::Rectangle);

        s.set_tool(Tool::Crop);
        drag(&mut s, pt(100.0, 100.0), pt(120.0, 100.0));
        s.key(Key::Escape, NONE);
        assert_eq!(s.doc().crop(), Some(Rect::new(50.0, 40.0, 350.0, 260.0)));
    }

    #[test]
    fn escape_backs_out_one_level_at_a_time() {
        let mut s = session();
        drag(&mut s, pt(10.0, 10.0), pt(100.0, 80.0));
        assert_eq!(s.key(Key::Escape, NONE), Outcome::Redraw);
        assert_eq!(s.key(Key::Escape, NONE), Outcome::Close);
        assert_eq!(s.key(Key::Enter, NONE), Outcome::Done);
    }

    #[test]
    fn copies_and_saves_are_tracked_per_state() {
        let mut s = session();
        assert!(!s.has_unsaved_work() && !s.is_saved() && !s.is_copied());
        drag(&mut s, pt(10.0, 10.0), pt(100.0, 80.0));
        assert!(s.has_unsaved_work());
        s.mark_copied();
        assert!(s.is_copied() && !s.is_saved() && !s.has_unsaved_work());
        drag(&mut s, pt(10.0, 200.0), pt(100.0, 280.0));
        assert!(!s.is_copied() && s.has_unsaved_work());
        s.mark_saved();
        assert!(s.is_saved());
        // Back to what was copied: copied again, but not saved.
        s.undo();
        assert!(s.is_copied() && !s.is_saved());

        let opened = Session::new(Document::new(&Image::new(4, 4, PixelFormat::Rgbx), 1.0), Style::default(), true);
        assert!(opened.is_saved());
    }

    #[test]
    fn pressing_the_redaction_key_again_switches_mode() {
        let mut s = session();
        s.key(Key::Text("b".into()), NONE);
        assert_eq!((s.tool(), s.redaction()), (Tool::Redact, Redaction::Pixelate));
        s.key(Key::Text("b".into()), NONE);
        assert_eq!(s.redaction(), Redaction::Blur);
    }
}
