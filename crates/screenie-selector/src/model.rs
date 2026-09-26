//! The selector's interaction logic, independent of GTK so it can be tested directly.
//!
//! All coordinates are global logical coordinates. The UI layer feeds pointer and key
//! events in and draws whatever [`Model`] says; an [`Outcome`] tells it when the user has
//! confirmed or cancelled.

use screenie_core::{OutputInfo, Point, Rect, WindowInfo};

/// What the user is picking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Drag a region; a plain click picks the window (or screen) under the cursor.
    #[default]
    Area,
    /// Click a window.
    Window,
    /// Click a screen.
    Screen,
}

/// Why the user is picking. Recordings always get a confirmation step because the
/// selection is live and the user usually wants to fine-tune it before starting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Purpose {
    #[default]
    Screenshot,
    Recording,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Region(Rect),
    Window(WindowInfo),
    Output(OutputInfo),
}

impl Selection {
    pub fn rect(&self) -> Rect {
        match self {
            Selection::Region(r) => *r,
            Selection::Window(w) => w.rect,
            Selection::Output(o) => o.logical,
        }
    }
}

/// Parts of a selection that can be grabbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Inside,
}

impl Handle {
    pub const RESIZE: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Where this handle sits on `r`.
    pub fn position(self, r: &Rect) -> Point {
        let (x0, x1, xm) = (r.x, r.right(), r.x + r.width / 2.0);
        let (y0, y1, ym) = (r.y, r.bottom(), r.y + r.height / 2.0);
        match self {
            Handle::TopLeft => Point::new(x0, y0),
            Handle::Top => Point::new(xm, y0),
            Handle::TopRight => Point::new(x1, y0),
            Handle::Right => Point::new(x1, ym),
            Handle::BottomRight => Point::new(x1, y1),
            Handle::Bottom => Point::new(xm, y1),
            Handle::BottomLeft => Point::new(x0, y1),
            Handle::Left => Point::new(x0, ym),
            Handle::Inside => r.center(),
        }
    }

    /// CSS cursor name for hovering this handle.
    pub fn cursor(self) -> &'static str {
        match self {
            Handle::TopLeft | Handle::BottomRight => "nwse-resize",
            Handle::TopRight | Handle::BottomLeft => "nesw-resize",
            Handle::Top | Handle::Bottom => "ns-resize",
            Handle::Left | Handle::Right => "ew-resize",
            Handle::Inside => "move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Space,
    Left,
    Right,
    Up,
    Down,
    Tab,
    /// Switch mode directly.
    Mode(Mode),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Nothing visible changed.
    Nothing,
    Redraw,
    Confirm(Selection),
    Cancel,
}

/// Minimum pointer travel (logical px) for a press to count as a drag rather than a click.
const CLICK_SLOP: f64 = 4.0;
/// Distance (logical px) within which a handle can be grabbed.
const HANDLE_REACH: f64 = 10.0;

#[derive(Debug, Clone)]
struct Grab {
    handle: Handle,
    start: Point,
    original: Rect,
}

#[derive(Debug, Clone)]
enum Phase {
    Idle,
    Drawing { anchor: Point, current: Point },
    Editing { selection: Selection, grab: Option<Grab> },
}

pub struct Model {
    outputs: Vec<OutputInfo>,
    windows: Vec<WindowInfo>,
    purpose: Purpose,
    mode: Mode,
    capture_on_release: bool,
    window_snapping: bool,
    cursor: Option<Point>,
    press: Option<Point>,
    space_held: bool,
    modifiers: Modifiers,
    phase: Phase,
}

impl Model {
    pub fn new(outputs: Vec<OutputInfo>, windows: Vec<WindowInfo>, purpose: Purpose, mode: Mode) -> Self {
        Self {
            outputs,
            windows,
            purpose,
            mode,
            capture_on_release: true,
            window_snapping: true,
            cursor: None,
            press: None,
            space_held: false,
            modifiers: Modifiers::default(),
            phase: Phase::Idle,
        }
    }

    pub fn with_capture_on_release(mut self, on: bool) -> Self {
        self.capture_on_release = on;
        self
    }

    pub fn with_window_snapping(mut self, on: bool) -> Self {
        self.window_snapping = on;
        if !on {
            self.windows.clear();
        }
        self
    }

    /// Start with an existing selection being edited (e.g. the last region).
    pub fn with_selection(mut self, selection: Selection) -> Self {
        self.phase = Phase::Editing { selection, grab: None };
        self
    }

    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn cursor(&self) -> Option<Point> {
        self.cursor
    }

    pub fn outputs(&self) -> &[OutputInfo] {
        &self.outputs
    }

    pub fn is_drawing(&self) -> bool {
        matches!(self.phase, Phase::Drawing { .. })
    }

    pub fn is_grabbing(&self) -> bool {
        matches!(self.phase, Phase::Editing { grab: Some(_), .. })
    }

    /// The committed, editable selection, if any.
    pub fn editing(&self) -> Option<&Selection> {
        match &self.phase {
            Phase::Editing { selection, .. } => Some(selection),
            _ => None,
        }
    }

    /// The rectangle to show as selected: the one being drawn or edited.
    pub fn selection_rect(&self) -> Option<Rect> {
        match &self.phase {
            Phase::Drawing { anchor, current } => {
                let r = self.constrain(*anchor, *current);
                (!r.is_empty()).then_some(r)
            }
            Phase::Editing { selection, .. } => Some(self.visible(selection.rect())),
            Phase::Idle => None,
        }
    }

    /// What a click at the cursor would pick, for hover highlighting.
    pub fn hover_target(&self) -> Option<Selection> {
        match &self.phase {
            Phase::Idle => self.cursor.and_then(|p| self.target_at(p)),
            _ => None,
        }
    }

    /// The handle under the cursor while editing.
    pub fn hover_handle(&self) -> Option<Handle> {
        let Phase::Editing { selection, grab } = &self.phase else {
            return None;
        };
        if let Some(g) = grab {
            return Some(g.handle);
        }
        self.cursor.and_then(|p| handle_at(&selection.rect(), p))
    }

    pub fn output_at(&self, p: Point) -> Option<&OutputInfo> {
        self.outputs.iter().find(|o| o.logical.contains(p))
    }

    fn layout_bounds(&self) -> Rect {
        self.outputs.iter().map(|o| o.logical).reduce(|a, b| a.union(&b)).unwrap_or_default()
    }

    /// Clip a rectangle to what is on screen.
    fn visible(&self, r: Rect) -> Rect {
        r.intersection(&self.layout_bounds()).unwrap_or(r)
    }

    fn target_at(&self, p: Point) -> Option<Selection> {
        let output = || self.output_at(p).cloned().map(Selection::Output);
        match self.mode {
            Mode::Screen => output(),
            Mode::Window => self.window_at(p).map(Selection::Window).or_else(output),
            Mode::Area if self.window_snapping => self.window_at(p).map(Selection::Window),
            Mode::Area => None,
        }
    }

    fn window_at(&self, p: Point) -> Option<WindowInfo> {
        let w = self.windows.iter().find(|w| w.rect.contains(p))?;
        let mut w = w.clone();
        w.rect = self.visible(w.rect);
        Some(w)
    }

    /// Round a point to the physical pixel grid of the output it's on, so selections
    /// map to whole pixels even with fractional scaling.
    fn snap(&self, p: Point) -> Point {
        let Some(o) = self.output_at(p).or_else(|| self.nearest_output(p)) else {
            return p;
        };
        let s = o.scale.max(1e-3);
        let snap = |v: f64, origin: f64| ((v - origin) * s).round() / s + origin;
        Point::new(snap(p.x, o.logical.x), snap(p.y, o.logical.y))
    }

    fn nearest_output(&self, p: Point) -> Option<&OutputInfo> {
        self.outputs.iter().min_by(|a, b| {
            let da = a.logical.center().distance(p);
            let db = b.logical.center().distance(p);
            da.total_cmp(&db)
        })
    }

    /// Apply Shift (square) and Alt (from center) to a drag.
    fn constrain(&self, anchor: Point, current: Point) -> Rect {
        let (mut dx, mut dy) = (current.x - anchor.x, current.y - anchor.y);
        if self.modifiers.shift {
            let side = dx.abs().max(dy.abs());
            dx = side.copysign(if dx == 0.0 { 1.0 } else { dx });
            dy = side.copysign(if dy == 0.0 { 1.0 } else { dy });
        }
        let rect = if self.modifiers.alt {
            Rect::from_corners(anchor.offset(-dx, -dy), anchor.offset(dx, dy))
        } else {
            Rect::from_corners(anchor, anchor.offset(dx, dy))
        };
        self.fit(rect, anchor)
    }

    /// Keep a region on screen; recordings additionally stay on one output.
    fn fit(&self, rect: Rect, reference: Point) -> Rect {
        rect.intersection(&self.bounds_for(reference)).unwrap_or(Rect::new(reference.x, reference.y, 0.0, 0.0))
    }

    pub fn set_modifiers(&mut self, modifiers: Modifiers) -> Outcome {
        if modifiers == self.modifiers {
            return Outcome::Nothing;
        }
        self.modifiers = modifiers;
        if self.is_drawing() { Outcome::Redraw } else { Outcome::Nothing }
    }

    pub fn set_mode(&mut self, mode: Mode) -> Outcome {
        if mode == self.mode {
            return Outcome::Nothing;
        }
        self.mode = mode;
        if !matches!(self.phase, Phase::Editing { .. }) || mode != Mode::Area {
            self.phase = Phase::Idle;
        }
        Outcome::Redraw
    }

    pub fn pointer_left(&mut self) -> Outcome {
        if self.press.is_some() {
            return Outcome::Nothing; // grabs keep going
        }
        self.cursor = None;
        Outcome::Redraw
    }

    pub fn pointer_moved(&mut self, p: Point) -> Outcome {
        let previous = self.cursor.replace(p);
        let snapped = self.snap(p);
        match &self.phase {
            Phase::Drawing { anchor, .. } => {
                let mut anchor = *anchor;
                if self.space_held && let Some(prev) = previous {
                    // Space-drag moves the whole selection instead of resizing it.
                    anchor = anchor.offset(p.x - prev.x, p.y - prev.y);
                }
                self.phase = Phase::Drawing { anchor, current: snapped };
            }
            Phase::Editing { grab: Some(grab), .. } => {
                let grab = grab.clone();
                let next = drag_rect(&grab, snapped, self.modifiers.shift);
                let next = if grab.handle == Handle::Inside {
                    next.clamp_within(&self.bounds_for(grab.original.center()))
                } else {
                    self.fit(next, grab.original.center())
                };
                self.phase = Phase::Editing { selection: Selection::Region(next), grab: Some(grab) };
            }
            _ => {}
        }
        Outcome::Redraw
    }

    /// Where a selection referenced at `p` may go: anywhere on screen for screenshots,
    /// within one output for recordings.
    fn bounds_for(&self, p: Point) -> Rect {
        match self.purpose {
            Purpose::Recording => self
                .output_at(p)
                .or_else(|| self.nearest_output(p))
                .map(|o| o.logical)
                .unwrap_or_else(|| self.layout_bounds()),
            Purpose::Screenshot => self.layout_bounds(),
        }
    }

    pub fn pressed(&mut self, p: Point) -> Outcome {
        self.cursor = Some(p);
        self.press = Some(p);
        let snapped = self.snap(p);
        match &mut self.phase {
            Phase::Editing { selection, grab } => {
                let rect = selection.rect();
                if let Some(handle) = handle_at(&rect, p) {
                    *grab = Some(Grab { handle, start: snapped, original: rect });
                    return Outcome::Redraw;
                }
                if self.mode == Mode::Area {
                    self.phase = Phase::Drawing { anchor: snapped, current: snapped };
                }
                Outcome::Redraw
            }
            Phase::Idle | Phase::Drawing { .. } if self.mode == Mode::Area => {
                self.phase = Phase::Drawing { anchor: snapped, current: snapped };
                Outcome::Redraw
            }
            _ => Outcome::Nothing,
        }
    }

    pub fn released(&mut self, p: Point) -> Outcome {
        self.cursor = Some(p);
        // Only a press that started here counts: releasing after clicking the toolbar
        // (e.g. its Window button) must not pick whatever is under the pointer.
        let Some(press) = self.press.take() else { return Outcome::Nothing };
        let is_click = press.distance(p) < CLICK_SLOP;
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Drawing { anchor, current } => {
                if is_click {
                    return self.click(p);
                }
                let rect = self.constrain(anchor, current);
                if rect.width < 1.0 || rect.height < 1.0 {
                    return Outcome::Redraw;
                }
                self.choose(Selection::Region(rect), self.capture_on_release)
            }
            Phase::Editing { selection, grab } => {
                self.phase = Phase::Editing { selection, grab: None };
                if grab.is_some() { Outcome::Redraw } else { Outcome::Nothing }
            }
            Phase::Idle => {
                if is_click {
                    self.click(p)
                } else {
                    Outcome::Nothing
                }
            }
        }
    }

    pub fn double_clicked(&mut self, p: Point) -> Outcome {
        match &self.phase {
            Phase::Editing { selection, .. } if selection.rect().contains(p) => Outcome::Confirm(selection.clone()),
            _ => Outcome::Nothing,
        }
    }

    /// A click (press and release without dragging) at `p`.
    fn click(&mut self, p: Point) -> Outcome {
        let target = self.target_at(p).or_else(|| {
            // In area mode without a window under the cursor, a click picks the screen.
            self.output_at(p).cloned().map(Selection::Output)
        });
        match target {
            // Clicking a window or screen is explicit enough to capture right away.
            Some(t) => self.choose(t, true),
            None => Outcome::Redraw,
        }
    }

    /// Confirm or start editing `selection`, depending on purpose and preference.
    fn choose(&mut self, selection: Selection, immediate: bool) -> Outcome {
        if self.purpose == Purpose::Screenshot && immediate {
            Outcome::Confirm(selection)
        } else {
            self.phase = Phase::Editing { selection, grab: None };
            Outcome::Redraw
        }
    }

    /// Confirm the current selection, or whatever Enter should pick when there is none.
    pub fn confirm(&mut self) -> Outcome {
        match &self.phase {
            Phase::Editing { selection, .. } => Outcome::Confirm(selection.clone()),
            Phase::Drawing { .. } => Outcome::Nothing,
            Phase::Idle => {
                let p = self.cursor.or_else(|| self.outputs.first().map(|o| o.logical.center()));
                let target = p.and_then(|p| match self.mode {
                    Mode::Window => self.window_at(p).map(Selection::Window),
                    _ => None,
                });
                let target = target.or_else(|| p.and_then(|p| self.output_at(p)).cloned().map(Selection::Output));
                target.map(Outcome::Confirm).unwrap_or(Outcome::Nothing)
            }
        }
    }

    pub fn key_pressed(&mut self, key: Key, modifiers: Modifiers) -> Outcome {
        self.modifiers = modifiers;
        match key {
            Key::Escape => Outcome::Cancel,
            Key::Enter => self.confirm(),
            Key::Space => {
                if self.is_drawing() {
                    self.space_held = true;
                    Outcome::Nothing
                } else if matches!(self.phase, Phase::Idle) {
                    // macOS muscle memory: space toggles between area and window.
                    let next = if self.mode == Mode::Window { Mode::Area } else { Mode::Window };
                    self.set_mode(next)
                } else {
                    Outcome::Nothing
                }
            }
            Key::Tab => {
                let next = match self.mode {
                    Mode::Area => Mode::Window,
                    Mode::Window => Mode::Screen,
                    Mode::Screen => Mode::Area,
                };
                self.set_mode(next)
            }
            Key::Mode(mode) => self.set_mode(mode),
            Key::Left | Key::Right | Key::Up | Key::Down => self.nudge(key, modifiers),
        }
    }

    pub fn key_released(&mut self, key: Key) -> Outcome {
        if key == Key::Space {
            self.space_held = false;
        }
        Outcome::Nothing
    }

    /// Arrow keys move the edited selection by one physical pixel (ten with Shift);
    /// with Ctrl they grow or shrink it from the bottom-right corner.
    fn nudge(&mut self, key: Key, modifiers: Modifiers) -> Outcome {
        let bounds = self.layout_bounds();
        let Phase::Editing { selection, grab: None } = &mut self.phase else {
            return Outcome::Nothing;
        };
        let rect = selection.rect();
        let scale = self
            .outputs
            .iter()
            .find(|o| o.logical.contains(rect.center()))
            .map(|o| o.scale)
            .unwrap_or(1.0);
        let step = if modifiers.shift { 10.0 } else { 1.0 } / scale;
        let (dx, dy) = match key {
            Key::Left => (-step, 0.0),
            Key::Right => (step, 0.0),
            Key::Up => (0.0, -step),
            Key::Down => (0.0, step),
            _ => return Outcome::Nothing,
        };
        let next = if modifiers.ctrl {
            let w = (rect.width + dx).max(step);
            let h = (rect.height + dy).max(step);
            Rect::new(rect.x, rect.y, w, h).intersection(&bounds).unwrap_or(rect)
        } else {
            rect.translate(dx, dy).clamp_within(&bounds)
        };
        *selection = Selection::Region(next);
        Outcome::Redraw
    }
}

/// The handle of `r` at `p`, preferring corners, then edges, then the inside.
pub fn handle_at(r: &Rect, p: Point) -> Option<Handle> {
    // Small selections get proportionally smaller handle zones so the inside stays
    // grabbable.
    let reach = HANDLE_REACH.min(r.width / 3.0).min(r.height / 3.0).max(4.0);
    let near = |h: Handle| h.position(r).distance(p) <= reach;
    for h in [Handle::TopLeft, Handle::TopRight, Handle::BottomRight, Handle::BottomLeft] {
        if near(h) {
            return Some(h);
        }
    }
    let within_x = p.x >= r.x - reach && p.x <= r.right() + reach;
    let within_y = p.y >= r.y - reach && p.y <= r.bottom() + reach;
    if within_y && (p.x - r.x).abs() <= reach {
        return Some(Handle::Left);
    }
    if within_y && (p.x - r.right()).abs() <= reach {
        return Some(Handle::Right);
    }
    if within_x && (p.y - r.y).abs() <= reach {
        return Some(Handle::Top);
    }
    if within_x && (p.y - r.bottom()).abs() <= reach {
        return Some(Handle::Bottom);
    }
    r.contains(p).then_some(Handle::Inside)
}

/// The rectangle resulting from dragging `grab` to `p`. Edges may cross over; the
/// result is normalized.
fn drag_rect(grab: &Grab, p: Point, keep_aspect: bool) -> Rect {
    let o = grab.original;
    let (dx, dy) = (p.x - grab.start.x, p.y - grab.start.y);
    let (mut x0, mut y0, mut x1, mut y1) = (o.x, o.y, o.right(), o.bottom());
    match grab.handle {
        Handle::Inside => return o.translate(dx, dy),
        Handle::TopLeft => (x0, y0) = (x0 + dx, y0 + dy),
        Handle::Top => y0 += dy,
        Handle::TopRight => (x1, y0) = (x1 + dx, y0 + dy),
        Handle::Right => x1 += dx,
        Handle::BottomRight => (x1, y1) = (x1 + dx, y1 + dy),
        Handle::Bottom => y1 += dy,
        Handle::BottomLeft => (x0, y1) = (x0 + dx, y1 + dy),
        Handle::Left => x0 += dx,
    }
    let mut r = Rect::from_corners(Point::new(x0, y0), Point::new(x1, y1));
    if keep_aspect && o.height > 0.0 && o.width > 0.0 {
        let aspect = o.width / o.height;
        match grab.handle {
            Handle::Top | Handle::Bottom => r.width = r.height * aspect,
            _ => r.height = r.width / aspect,
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenie_core::Transform;

    fn output(name: &str, x: f64, w: f64, h: f64, scale: f64) -> OutputInfo {
        OutputInfo {
            name: name.into(),
            description: String::new(),
            logical: Rect::new(x, 0.0, w, h),
            scale,
            transform: Transform::Normal,
        }
    }

    fn window(title: &str, rect: Rect) -> WindowInfo {
        WindowInfo { id: title.into(), title: title.into(), app_id: title.into(), rect, focused: false, floating: false }
    }

    fn model(purpose: Purpose) -> Model {
        Model::new(
            vec![output("A", 0.0, 1920.0, 1080.0, 1.0), output("B", 1920.0, 1280.0, 720.0, 1.5)],
            vec![window("top", Rect::new(100.0, 100.0, 400.0, 300.0)), window("big", Rect::new(0.0, 0.0, 1920.0, 1080.0))],
            purpose,
            Mode::Area,
        )
    }

    const NO_MODS: Modifiers = Modifiers { shift: false, ctrl: false, alt: false };

    #[test]
    fn drag_captures_region_on_release() {
        let mut m = model(Purpose::Screenshot);
        m.pressed(Point::new(10.0, 20.0));
        m.pointer_moved(Point::new(110.0, 70.0));
        assert_eq!(m.selection_rect(), Some(Rect::new(10.0, 20.0, 100.0, 50.0)));
        assert_eq!(m.released(Point::new(110.0, 70.0)), Outcome::Confirm(Selection::Region(Rect::new(10.0, 20.0, 100.0, 50.0))));
    }

    #[test]
    fn click_picks_topmost_window() {
        let mut m = model(Purpose::Screenshot);
        m.pointer_moved(Point::new(150.0, 150.0));
        assert!(matches!(m.hover_target(), Some(Selection::Window(w)) if w.title == "top"));
        m.pressed(Point::new(150.0, 150.0));
        match m.released(Point::new(151.0, 150.0)) {
            Outcome::Confirm(Selection::Window(w)) => assert_eq!(w.title, "top"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn click_without_window_picks_screen() {
        let mut m = model(Purpose::Screenshot);
        m.pressed(Point::new(2000.0, 10.0));
        match m.released(Point::new(2000.0, 10.0)) {
            Outcome::Confirm(Selection::Output(o)) => assert_eq!(o.name, "B"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn adjust_mode_keeps_selection_until_confirmed() {
        let mut m = model(Purpose::Screenshot).with_capture_on_release(false);
        m.pressed(Point::new(0.0, 0.0));
        m.pointer_moved(Point::new(50.0, 50.0));
        assert_eq!(m.released(Point::new(50.0, 50.0)), Outcome::Redraw);
        assert!(m.editing().is_some());
        // Drag the bottom-right handle outward.
        m.pressed(Point::new(50.0, 50.0));
        m.pointer_moved(Point::new(80.0, 60.0));
        m.released(Point::new(80.0, 60.0));
        assert_eq!(m.selection_rect(), Some(Rect::new(0.0, 0.0, 80.0, 60.0)));
        assert_eq!(m.key_pressed(Key::Enter, NO_MODS), Outcome::Confirm(Selection::Region(Rect::new(0.0, 0.0, 80.0, 60.0))));
    }

    #[test]
    fn recording_regions_stay_on_one_output() {
        let mut m = model(Purpose::Recording);
        m.pressed(Point::new(1800.0, 100.0));
        m.pointer_moved(Point::new(2100.0, 200.0));
        m.released(Point::new(2100.0, 200.0));
        assert_eq!(m.selection_rect(), Some(Rect::new(1800.0, 100.0, 120.0, 100.0)));
    }

    #[test]
    fn screenshots_may_span_outputs() {
        let mut m = model(Purpose::Screenshot);
        m.pressed(Point::new(1800.0, 100.0));
        m.pointer_moved(Point::new(2100.0, 200.0));
        assert_eq!(m.selection_rect(), Some(Rect::new(1800.0, 100.0, 300.0, 100.0)));
    }

    #[test]
    fn shift_makes_square() {
        let mut m = model(Purpose::Screenshot);
        m.pressed(Point::new(100.0, 100.0));
        m.set_modifiers(Modifiers { shift: true, ..NO_MODS });
        m.pointer_moved(Point::new(150.0, 120.0));
        assert_eq!(m.selection_rect(), Some(Rect::new(100.0, 100.0, 50.0, 50.0)));
    }

    #[test]
    fn space_moves_while_drawing() {
        let mut m = model(Purpose::Screenshot);
        m.pressed(Point::new(100.0, 100.0));
        m.pointer_moved(Point::new(200.0, 200.0));
        m.key_pressed(Key::Space, NO_MODS);
        m.pointer_moved(Point::new(250.0, 210.0));
        assert_eq!(m.selection_rect(), Some(Rect::new(150.0, 110.0, 100.0, 100.0)));
    }

    #[test]
    fn points_snap_to_physical_pixels() {
        let mut m = model(Purpose::Screenshot);
        // On the 1.5x output, 2000.3 logical is 120.45 physical from its origin: rounds to
        // 120 physical = 80 logical.
        m.pressed(Point::new(2000.3, 10.0));
        m.pointer_moved(Point::new(2100.0, 110.0));
        assert_eq!(m.selection_rect().unwrap().x, 2000.0);
    }

    #[test]
    fn a_release_without_a_press_picks_nothing() {
        // Clicking a toolbar button: the press lands on the button, only the release
        // reaches the canvas.
        let mut m = model(Purpose::Screenshot);
        m.set_mode(Mode::Window);
        m.pointer_moved(Point::new(150.0, 150.0));
        assert_eq!(m.released(Point::new(150.0, 150.0)), Outcome::Nothing);
        assert_eq!(m.mode(), Mode::Window);
    }

    #[test]
    fn space_toggles_window_mode_when_idle() {
        let mut m = model(Purpose::Screenshot);
        m.key_pressed(Key::Space, NO_MODS);
        assert_eq!(m.mode(), Mode::Window);
    }

    #[test]
    fn enter_without_selection_picks_screen_under_cursor() {
        let mut m = model(Purpose::Screenshot);
        m.pointer_moved(Point::new(2500.0, 100.0));
        match m.key_pressed(Key::Enter, NO_MODS) {
            Outcome::Confirm(Selection::Output(o)) => assert_eq!(o.name, "B"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn handles() {
        let r = Rect::new(100.0, 100.0, 200.0, 100.0);
        assert_eq!(handle_at(&r, Point::new(101.0, 99.0)), Some(Handle::TopLeft));
        assert_eq!(handle_at(&r, Point::new(200.0, 199.0)), Some(Handle::Bottom));
        assert_eq!(handle_at(&r, Point::new(200.0, 150.0)), Some(Handle::Inside));
        assert_eq!(handle_at(&r, Point::new(500.0, 150.0)), None);
    }
}
