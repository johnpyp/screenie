//! Which output the user is on, asked of the compositor itself.
//!
//! Wayland has no request for "the focused output". But a layer surface opened without an
//! output is put, in the protocol's words, on the one "the user most recently interacted
//! with", and `wl_surface.enter` then names it. So [`focused_output`] maps a single
//! transparent pixel that takes no input, reads where it lands, and removes it. That works
//! on every compositor with layer-shell (sway, Hyprland, niri, river, Wayfire, labwc,
//! COSMIC) and needs no IPC: compositors with IPC answer faster through it.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use crate::{Error, Result};

/// How long the compositor gets to place the probe.
const TIMEOUT: Duration = Duration::from_millis(250);

/// The connector name of the output the user is on, as the compositor sees it. `None`
/// without layer-shell, or if the compositor doesn't say in time.
pub fn focused_output() -> Result<Option<String>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Probe>(&conn)?;
    let qh = queue.handle();
    let (Ok(compositor), Ok(shm), Ok(layer_shell)) = (
        globals.bind::<wl_compositor::WlCompositor, _, _>(&qh, 1..=4, ()),
        globals.bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ()),
        globals.bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ()),
    ) else {
        return Ok(None);
    };
    let mut probe = Probe::default();
    bind_outputs(&globals, &qh, &mut probe);

    let surface = compositor.create_surface(&qh, ());
    let region = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&region));
    let layer = layer_shell.get_layer_surface(
        &surface,
        None,
        Layer::Overlay,
        "screenie-probe".into(),
        &qh,
        (),
    );
    layer.set_size(1, 1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    surface.commit();
    let buffer = transparent_pixel(&shm, &qh)?;

    let deadline = Instant::now() + TIMEOUT;
    let mut attached = false;
    let found = loop {
        if let Some(i) = probe.entered {
            break probe.outputs.get(i).map(|o| o.name(i));
        }
        if probe.closed {
            break None;
        }
        if let Some(serial) = probe.configured.take() {
            layer.ack_configure(serial);
            if !attached {
                surface.attach(Some(&buffer), 0, 0);
                surface.damage(0, 0, 1, 1);
                attached = true;
            }
            surface.commit();
        }
        let now = Instant::now();
        if now >= deadline {
            tracing::debug!("the compositor didn't place the focus probe in time");
            break None;
        }
        dispatch_timeout(&mut queue, &mut probe, deadline - now)?;
    };

    layer.destroy();
    surface.destroy();
    buffer.destroy();
    region.destroy();
    let _ = conn.flush();
    Ok(found)
}

#[derive(Default)]
struct Probe {
    outputs: Vec<ProbedOutput>,
    /// The output the probe entered (an index into `outputs`).
    entered: Option<usize>,
    /// A configure to acknowledge.
    configured: Option<u32>,
    closed: bool,
}

struct ProbedOutput {
    wl_output: wl_output::WlOutput,
    /// Held so its events keep coming.
    #[allow(dead_code)]
    xdg_output: Option<ZxdgOutputV1>,
    name: Option<String>,
}

impl ProbedOutput {
    /// Named as `Capturer::outputs` names it.
    fn name(&self, index: usize) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("output-{index}"))
    }
}

/// Bind every output, in the order `Capturer` does (so unnamed ones match).
fn bind_outputs(globals: &GlobalList, qh: &QueueHandle<Probe>, probe: &mut Probe) {
    let xdg: Option<ZxdgOutputManagerV1> = globals.bind(qh, 2..=3, ()).ok();
    let output_globals: Vec<_> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .cloned()
            .collect()
    });
    for global in output_globals {
        let index = probe.outputs.len();
        let wl_output: wl_output::WlOutput =
            globals
                .registry()
                .bind(global.name, global.version.min(4), qh, index);
        let xdg_output = xdg
            .as_ref()
            .map(|m| m.get_xdg_output(&wl_output, qh, index));
        probe.outputs.push(ProbedOutput {
            wl_output,
            xdg_output,
            name: None,
        });
    }
}

/// A 1×1 fully transparent buffer.
fn transparent_pixel(shm: &wl_shm::WlShm, qh: &QueueHandle<Probe>) -> Result<wl_buffer::WlBuffer> {
    let fd = rustix::fs::memfd_create(
        "screenie-probe",
        rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
    )
    .map_err(std::io::Error::from)?;
    rustix::fs::ftruncate(&fd, 4).map_err(std::io::Error::from)?;
    let pool = shm.create_pool(fd.as_fd(), 4, qh, ());
    let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    Ok(buffer)
}

/// Dispatch events, waiting at most `timeout` for new ones.
fn dispatch_timeout(
    queue: &mut EventQueue<Probe>,
    probe: &mut Probe,
    timeout: Duration,
) -> Result<()> {
    if queue.dispatch_pending(probe)? > 0 {
        return Ok(());
    }
    queue.flush()?;
    let Some(guard) = queue.prepare_read() else {
        queue.dispatch_pending(probe)?;
        return Ok(());
    };
    let readable = {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let fd = guard.connection_fd();
        let mut fds = [PollFd::new(&fd, PollFlags::IN | PollFlags::ERR)];
        let ts = Timespec {
            tv_sec: timeout.as_secs() as _,
            tv_nsec: timeout.subsec_nanos() as _,
        };
        loop {
            match poll(&mut fds, Some(&ts)) {
                Ok(n) => break n > 0,
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(Error::Io(e.into())),
            }
        }
    };
    if readable {
        match guard.read() {
            Ok(_) => {}
            Err(wayland_client::backend::WaylandError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        queue.dispatch_pending(probe)?;
    }
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Probe {
    fn event(
        probe: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            probe.outputs[*index].name = Some(name);
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for Probe {
    fn event(
        probe: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::Name { name } = event {
            probe.outputs[*index].name.get_or_insert(name);
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::Enter { output } = event {
            probe.entered = probe
                .outputs
                .iter()
                .position(|o| o.wl_output.id() == output.id());
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                probe.configured = Some(serial)
            }
            zwlr_layer_surface_v1::Event::Closed => probe.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Every compositor supports ARGB8888, the only format used.
    }
}

delegate_noop!(Probe: ignore wl_compositor::WlCompositor);
delegate_noop!(Probe: ignore wl_region::WlRegion);
delegate_noop!(Probe: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Probe: ignore wl_buffer::WlBuffer);
delegate_noop!(Probe: ignore ZwlrLayerShellV1);
delegate_noop!(Probe: ignore ZxdgOutputManagerV1);
