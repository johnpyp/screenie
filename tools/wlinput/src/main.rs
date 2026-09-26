//! Drive a wlroots compositor's pointer and keyboard for UI testing.
//!
//! Coordinates are global logical coordinates (the compositor layout). Commands run in
//! order; each is one argument group:
//!
//! ```text
//! wlinput move X Y                 absolute pointer motion
//! wlinput down [left|right|middle] press a button
//! wlinput up [left|right|middle]   release a button
//! wlinput click X Y                move, press, release
//! wlinput drag X1 Y1 X2 Y2 [STEPS] press at 1, glide to 2, release
//! wlinput key NAME...              tap keys (escape enter space tab left right up down, letters, digits)
//! wlinput hold NAME / release NAME hold or release one key (shift, ctrl, alt, super)
//! wlinput scroll N                 N wheel clicks (negative: up)
//! wlinput sleep MS
//! ```
//! Several commands can be chained: `wlinput move 10 10 , sleep 100 , click 50 50`.
//!
//! The virtual pointer goes away when wlinput exits, and the compositor then sends the
//! surface under it a pointer leave. To test hover, keep it alive while you look:
//! `wlinput move 10 10 , sleep 3000 &`, then take the screenshot.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

fn keycode(name: &str) -> Option<u32> {
    let letters = "qwertyuiop"
        .chars()
        .zip(16..)
        .chain("asdfghjkl".chars().zip(30..))
        .chain("zxcvbnm".chars().zip(44..));
    if name.len() == 1 {
        let c = name.chars().next()?.to_ascii_lowercase();
        if let Some((_, code)) = letters.clone().find(|(l, _)| *l == c) {
            return Some(code);
        }
        if let Some(d) = c.to_digit(10) {
            return Some(if d == 0 { 11 } else { d + 1 });
        }
    }
    Some(match name {
        "escape" | "esc" => 1,
        "enter" | "return" => 28,
        "space" => 57,
        "tab" => 15,
        "backspace" => 14,
        "delete" => 111,
        "left" => 105,
        "right" => 106,
        "up" => 103,
        "down" => 108,
        "shift" => 42,
        "ctrl" | "control" => 29,
        "alt" => 56,
        "super" => 125,
        _ => return None,
    })
}

fn button(name: Option<&str>) -> u32 {
    match name {
        Some("right") => BTN_RIGHT,
        Some("middle") => BTN_MIDDLE,
        _ => BTN_LEFT,
    }
}

struct Input {
    conn: Connection,
    pointer: ZwlrVirtualPointerV1,
    keyboard: Option<ZwpVirtualKeyboardV1>,
    extent: (f64, f64, f64, f64),
    start: Instant,
    pos: (f64, f64),
    /// Held modifiers (xkb mask). Virtual keyboards report these themselves.
    mods: u32,
}

impl Input {
    fn time(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn flush(&self) {
        let _ = self.conn.flush();
    }

    fn move_to(&mut self, x: f64, y: f64) {
        let (ox, oy, w, h) = self.extent;
        let t = self.time();
        self.pointer.motion_absolute(t, (x - ox).max(0.0) as u32, (y - oy).max(0.0) as u32, w as u32, h as u32);
        self.pointer.frame();
        self.pos = (x, y);
        self.flush();
    }

    fn scroll(&mut self, clicks: i32) {
        let time = self.time();
        self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
        self.pointer.axis_discrete(time, wl_pointer::Axis::VerticalScroll, 15.0 * clicks as f64, clicks);
        self.pointer.frame();
        self.flush();
    }

    fn button(&mut self, button: u32, pressed: bool) {
        let t = self.time();
        let state = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
        self.pointer.button(t, button, state);
        self.pointer.frame();
        self.flush();
    }

    fn key(&mut self, code: u32, pressed: bool) {
        let Some(kb) = &self.keyboard else {
            eprintln!("no virtual keyboard support");
            return;
        };
        kb.key(self.time(), code, if pressed { 1 } else { 0 });
        // The us keymap's modifier bits: Shift, Control, Mod1 (Alt), Mod4 (Super).
        let bit = match code {
            42 => 1,
            29 => 4,
            56 => 8,
            125 => 64,
            _ => 0,
        };
        if bit != 0 {
            self.mods = if pressed { self.mods | bit } else { self.mods & !bit };
            kb.modifiers(self.mods, 0, 0, 0);
        }
        self.flush();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: see the module docs in tools/wlinput/src/main.rs");
        std::process::exit(2);
    }

    // The layout extent maps absolute motion onto global coordinates.
    let outputs = screenie_wayland::Capturer::connect().map(|c| c.outputs()).unwrap_or_default();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for o in &outputs {
        x0 = x0.min(o.logical.x);
        y0 = y0.min(o.logical.y);
        x1 = x1.max(o.logical.right());
        y1 = y1.max(o.logical.bottom());
    }
    if outputs.is_empty() {
        (x0, y0, x1, y1) = (0.0, 0.0, 1920.0, 1080.0);
    }

    let conn = Connection::connect_to_env().expect("wayland connection");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).expect("seat");
    let pointer_manager: ZwlrVirtualPointerManagerV1 =
        globals.bind(&qh, 1..=2, ()).expect("compositor lacks zwlr_virtual_pointer_manager_v1");
    let pointer = pointer_manager.create_virtual_pointer(Some(&seat), &qh, ());
    let keyboard = globals.bind::<ZwpVirtualKeyboardManagerV1, _, _>(&qh, 1..=1, ()).ok().map(|m| {
        let kb = m.create_virtual_keyboard(&seat, &qh, ());
        let ctx = xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
        let keymap = xkbcommon::xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS)
            .expect("us keymap");
        let text = keymap.get_as_string(xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1);
        let fd = rustix::fs::memfd_create("keymap", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd");
        let file = std::fs::File::from(fd);
        use std::io::Write;
        (&file).write_all(text.as_bytes()).expect("write keymap");
        (&file).write_all(&[0]).expect("write keymap");
        kb.keymap(1, file.as_fd(), text.len() as u32 + 1);
        kb
    });
    queue.roundtrip(&mut State).expect("roundtrip");
    // Clients drop the first key from a keyboard whose keymap they haven't seen yet, so
    // introduce it with a no-op modifiers event first.
    if let Some(kb) = &keyboard {
        kb.modifiers(0, 0, 0, 0);
        queue.roundtrip(&mut State).expect("roundtrip");
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut input = Input {
        conn: conn.clone(),
        pointer,
        keyboard,
        extent: (x0, y0, x1 - x0, y1 - y0),
        start: Instant::now(),
        pos: (0.0, 0.0),
        mods: 0,
    };

    let num = |s: Option<&String>| -> f64 { s.and_then(|v| v.parse().ok()).expect("expected a number") };
    for group in args.split(|a| a == ",") {
        let Some(cmd) = group.first() else { continue };
        let rest = &group[1..];
        match cmd.as_str() {
            "move" => input.move_to(num(rest.first()), num(rest.get(1))),
            "down" => input.button(button(rest.first().map(String::as_str)), true),
            "up" => input.button(button(rest.first().map(String::as_str)), false),
            "click" => {
                input.move_to(num(rest.first()), num(rest.get(1)));
                std::thread::sleep(Duration::from_millis(30));
                input.button(BTN_LEFT, true);
                std::thread::sleep(Duration::from_millis(30));
                input.button(BTN_LEFT, false);
            }
            "drag" => {
                let (ax, ay, bx, by) = (num(rest.first()), num(rest.get(1)), num(rest.get(2)), num(rest.get(3)));
                let steps = rest.get(4).and_then(|s| s.parse().ok()).unwrap_or(12u32);
                input.move_to(ax, ay);
                std::thread::sleep(Duration::from_millis(40));
                input.button(BTN_LEFT, true);
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    std::thread::sleep(Duration::from_millis(16));
                    input.move_to(ax + (bx - ax) * t, ay + (by - ay) * t);
                }
                std::thread::sleep(Duration::from_millis(40));
                input.button(BTN_LEFT, false);
            }
            "key" => {
                for name in rest {
                    let code = keycode(name).unwrap_or_else(|| panic!("unknown key {name}"));
                    input.key(code, true);
                    std::thread::sleep(Duration::from_millis(20));
                    input.key(code, false);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            "hold" | "release" => {
                let name = rest.first().expect("key name");
                let code = keycode(name).unwrap_or_else(|| panic!("unknown key {name}"));
                input.key(code, cmd == "hold");
            }
            "scroll" => {
                let clicks = num(rest.first()) as i32;
                for _ in 0..clicks.abs() {
                    input.scroll(clicks.signum());
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
            "sleep" => std::thread::sleep(Duration::from_millis(num(rest.first()) as u64)),
            other => panic!("unknown command {other}"),
        }
    }
    let _ = queue.roundtrip(&mut State);
    let _ = input.pos;
}
