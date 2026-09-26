//! A stand-in for a fullscreen game: a fullscreen window that locks the pointer on
//! request (`lock` / `unlock` on stdin, each answered `ok` once the compositor has it)
//! and prints its focus, lock and relative motion events, a line each. See the README.

use std::io::{BufRead, Write};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_locked_pointer_v1::{self, ZwpLockedPointerV1},
    zwp_pointer_constraints_v1::{Lifetime, ZwpPointerConstraintsV1},
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

struct State {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    shown: bool,
    running: bool,
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

impl State {
    /// A solid buffer of the configured size.
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let (w, h) = (self.size.0.max(1), self.size.1.max(1));
        let stride = w * 4;
        let fd = memfd_create("wllock", MemfdFlags::CLOEXEC).expect("memfd");
        let mut file = std::fs::File::from(fd);
        // 0xff2e3440, little-endian ARGB.
        let pixels: Vec<u8> = [0x40, 0x34, 0x2e, 0xff].repeat((w * h) as usize);
        file.write_all(&pixels).expect("writing the buffer");
        let pool = self.shm.create_pool(file.as_fd(), stride * h, qh, ());
        let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, w, h);
        self.surface.commit();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(_: &mut Self, base: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(state: &mut Self, surface: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.draw(qh);
            if !state.shown {
                state.shown = true;
                say("ready");
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(state: &mut Self, _: &xdg_toplevel::XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 => state.size = (width, height),
            xdg_toplevel::Event::Close => state.running = false,
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(_: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { .. } => say("pointer enter"),
            wl_pointer::Event::Leave { .. } => say("pointer leave"),
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(_: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { .. } => say("keyboard enter"),
            wl_keyboard::Event::Leave { .. } => say("keyboard leave"),
            _ => {}
        }
    }
}

impl Dispatch<ZwpRelativePointerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwpRelativePointerV1, event: zwp_relative_pointer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion { dx_unaccel, dy_unaccel, .. } = event {
            say(&format!("motion {dx_unaccel} {dy_unaccel}"));
        }
    }
}

impl Dispatch<ZwpLockedPointerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwpLockedPointerV1, event: zwp_locked_pointer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwp_locked_pointer_v1::Event::Locked => say("locked"),
            zwp_locked_pointer_v1::Event::Unlocked => say("unlocked"),
            _ => {}
        }
    }
}

/// A command's round trip: the compositor has handled it.
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(_: &mut Self, _: &wl_callback::WlCallback, event: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = event {
            say("ok");
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: ZwpPointerConstraintsV1);
delegate_noop!(State: ZwpRelativePointerManagerV1);

fn main() {
    let conn = Connection::connect_to_env().expect("no Wayland compositor");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).expect("xdg_wm_base");
    let seat: wl_seat::WlSeat = globals.bind(&qh, 5..=9, ()).expect("wl_seat");
    let constraints: ZwpPointerConstraintsV1 = globals.bind(&qh, 1..=1, ()).expect("pointer constraints");
    let relative: ZwpRelativePointerManagerV1 = globals.bind(&qh, 1..=1, ()).expect("relative pointer");

    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("wllock".into());
    toplevel.set_app_id("wllock".into());
    toplevel.set_fullscreen(None);
    surface.commit();

    let pointer = seat.get_pointer(&qh, ());
    let _keyboard = seat.get_keyboard(&qh, ());
    let _relative = relative.get_relative_pointer(&pointer, &qh, ());

    let locked: Arc<Mutex<Option<ZwpLockedPointerV1>>> = Arc::default();
    {
        let (conn, qh, surface, locked) = (conn.clone(), qh.clone(), surface.clone(), locked.clone());
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let mut locked = locked.lock().unwrap();
                match line.trim() {
                    "lock" => {
                        if locked.is_none() {
                            *locked =
                                Some(constraints.lock_pointer(&surface, &pointer, None, Lifetime::Persistent, &qh, ()));
                        }
                    }
                    "unlock" => {
                        if let Some(l) = locked.take() {
                            l.destroy();
                        }
                    }
                    other => {
                        say(&format!("error: unknown command {other:?}"));
                        continue;
                    }
                }
                conn.display().sync(&qh, ());
                let _ = conn.flush();
            }
        });
    }

    let mut state = State { shm, surface, size: (640, 480), shown: false, running: true };
    while state.running {
        if queue.blocking_dispatch(&mut state).is_err() {
            break;
        }
    }
}
