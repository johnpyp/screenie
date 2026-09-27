//! KWin's own screen casts (`zkde_screencast_unstable_v1`): an output, a region of the
//! desktop or a window, cast to a PipeWire node on the session's daemon. It's what
//! Spectacle records with, and what KDE's portal starts once the user has picked.
//!
//! KWin offers the protocol only to clients it trusts: up to Plasma 6.7, those whose
//! desktop file lists it under `X-KDE-Wayland-Interfaces` (see screenie-desktop); from
//! 6.8, every client that isn't sandboxed.

use std::time::{Duration, Instant};

use screenie_core::Rect;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_plasma::screencast::v1::client::{
    zkde_screencast_stream_unstable_v1::{self, ZkdeScreencastStreamUnstableV1},
    zkde_screencast_unstable_v1::{Pointer, ZkdeScreencastUnstableV1},
};

use crate::{Error, Result};

/// How long KWin gets to make the node.
const TIMEOUT: Duration = Duration::from_secs(3);

/// What to cast.
#[derive(Debug, Clone)]
pub enum KdeSource {
    /// An output, by connector name.
    Output(String),
    /// A logical area of the desktop, at the highest scale of the outputs it covers.
    Region(Rect),
    /// A window, by KWin's UUID for it (`internalId`).
    Window(String),
}

/// How the cast shows the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdePointer {
    Hidden,
    Embedded,
    /// Beside the frames, as PipeWire metadata.
    Metadata,
}

/// A running cast. Closed when dropped.
pub struct KdeCast {
    /// Its PipeWire node, on the session's daemon.
    pub node: u32,
    stream: ZkdeScreencastStreamUnstableV1,
    conn: Connection,
}

impl Drop for KdeCast {
    fn drop(&mut self) {
        self.stream.close();
        let _ = self.conn.flush();
    }
}

/// Whether KWin lets this client cast.
pub fn kde_screencast_available() -> bool {
    Connection::connect_to_env()
        .ok()
        .and_then(|conn| registry_queue_init::<Caster>(&conn).ok())
        .is_some_and(|(globals, _)| {
            globals.contents().with_list(|list| {
                list.iter()
                    .any(|g| g.interface == ZkdeScreencastUnstableV1::interface().name)
            })
        })
}

/// Ask KWin to cast `source`. Blocks until its PipeWire node exists.
pub fn kde_cast(source: &KdeSource, pointer: KdePointer) -> Result<KdeCast> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Caster>(&conn)?;
    let qh = queue.handle();
    let manager: ZkdeScreencastUnstableV1 = globals.bind(&qh, 1..=4, ()).map_err(|_| {
        Error::Unsupported("KWin doesn't let screenie cast the screen (zkde_screencast)".into())
    })?;
    let pointer = match pointer {
        KdePointer::Hidden => Pointer::Hidden,
        KdePointer::Embedded => Pointer::Embedded,
        KdePointer::Metadata => Pointer::Metadata,
    } as u32;
    let mut caster = Caster::default();
    let stream = match source {
        KdeSource::Output(name) => {
            // Outputs are known by name through xdg-output (or wl_output v4).
            let xdg: Option<ZxdgOutputManagerV1> = globals.bind(&qh, 2..=3, ()).ok();
            let outputs: Vec<_> = globals.contents().with_list(|list| {
                list.iter()
                    .filter(|g| g.interface == "wl_output")
                    .cloned()
                    .collect()
            });
            for (index, global) in outputs.into_iter().enumerate() {
                let output: wl_output::WlOutput =
                    globals
                        .registry()
                        .bind(global.name, global.version.min(4), &qh, index);
                if let Some(xdg) = &xdg {
                    xdg.get_xdg_output(&output, &qh, index);
                }
                caster.outputs.push((output, None));
            }
            queue.roundtrip(&mut caster)?;
            queue.roundtrip(&mut caster)?;
            let output = caster
                .outputs
                .iter()
                .find(|(_, n)| n.as_deref() == Some(name.as_str()))
                .map(|(o, _)| o.clone())
                .ok_or_else(|| Error::Unsupported(format!("no output named {name}")))?;
            manager.stream_output(&output, pointer, &qh, ())
        }
        KdeSource::Region(rect) => {
            if manager.version() < 3 {
                return Err(Error::Unsupported(
                    "this KWin can't cast a region of the desktop".into(),
                ));
            }
            // A scale of 0 is "the highest of the outputs it covers" from version 5; the
            // versions before want it spelled out, and 1 is what they did with 0.
            manager.stream_region(
                rect.x.round() as i32,
                rect.y.round() as i32,
                rect.width.round().max(1.0) as u32,
                rect.height.round().max(1.0) as u32,
                0.0,
                pointer,
                &qh,
                (),
            )
        }
        KdeSource::Window(uuid) => manager.stream_window(uuid.clone(), pointer, &qh, ()),
    };

    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(node) = caster.node {
            return Ok(KdeCast { node, stream, conn });
        }
        if let Some(why) = caster.failed.take() {
            stream.close();
            return Err(Error::Unsupported(format!("KWin couldn't cast it: {why}")));
        }
        let now = Instant::now();
        if now >= deadline {
            stream.close();
            return Err(Error::Timeout);
        }
        crate::dispatch_timeout(&mut queue, &mut caster, deadline - now)?;
    }
}

#[derive(Default)]
struct Caster {
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
    node: Option<u32>,
    failed: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Caster {
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

impl Dispatch<wl_output::WlOutput, usize> for Caster {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(output) = state.outputs.get_mut(*index)
        {
            output.1 = Some(name);
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for Caster {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::Name { name } = event
            && let Some(output) = state.outputs.get_mut(*index)
        {
            output.1.get_or_insert(name);
        }
    }
}

impl Dispatch<ZkdeScreencastStreamUnstableV1, ()> for Caster {
    fn event(
        state: &mut Self,
        _: &ZkdeScreencastStreamUnstableV1,
        event: zkde_screencast_stream_unstable_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zkde_screencast_stream_unstable_v1::Event::Created { node } => state.node = Some(node),
            zkde_screencast_stream_unstable_v1::Event::Failed { error } => {
                state.failed = Some(error)
            }
            zkde_screencast_stream_unstable_v1::Event::Closed => {
                state.failed = Some("KWin closed the cast".into())
            }
            _ => {}
        }
    }
}

delegate_noop!(Caster: ignore ZkdeScreencastUnstableV1);
delegate_noop!(Caster: ignore ZxdgOutputManagerV1);
