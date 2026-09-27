//! KDE Plasma: KWin's screenshots (`org.kde.KWin.ScreenShot2`), which Spectacle takes
//! them with. Each output comes at its own scale, written raw into a pipe.
//!
//! Up to Plasma 6.7, KWin only answers clients whose desktop file lists the interface
//! under `X-KDE-DBUS-Restricted-Interfaces` (screenie-desktop writes one); from 6.8,
//! every client that isn't sandboxed. Streams come from KWin's screen casts instead
//! (`screenie_wayland::kde_cast`).

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::OwnedFd;

use screenie_core::{Image, OutputCapture, OutputInfo, PixelFormat};
use zbus::Connection;
use zbus::zvariant::{Fd, OwnedValue, Value};

use crate::{Error, Result};

#[zbus::proxy(
    interface = "org.kde.KWin.ScreenShot2",
    default_service = "org.kde.KWin.ScreenShot2",
    default_path = "/org/kde/KWin/ScreenShot2",
    gen_blocking = false
)]
trait ScreenShot2 {
    #[zbus(property)]
    fn version(&self) -> zbus::Result<u32>;

    fn capture_screen(
        &self,
        name: &str,
        options: HashMap<&str, Value<'_>>,
        pipe: Fd<'_>,
    ) -> zbus::Result<HashMap<String, OwnedValue>>;
}

/// Whether KWin's screenshots are there (they may still refuse screenie, see above).
pub(crate) fn available() -> bool {
    async_io::block_on(async {
        let connection = Connection::session().await?;
        ScreenShot2Proxy::new(&connection).await?.version().await
    })
    .is_ok()
}

/// A still of each of `outputs`, taken at once.
pub(crate) fn capture(outputs: &[OutputInfo], cursor: bool) -> Result<Vec<OutputCapture>> {
    let connection = async_io::block_on(Connection::session()).map_err(dbus)?;
    std::thread::scope(|scope| {
        let shots: Vec<_> = outputs
            .iter()
            .map(|output| {
                let connection = connection.clone();
                scope.spawn(move || {
                    async_io::block_on(async {
                        let proxy = ScreenShot2Proxy::new(&connection).await.map_err(dbus)?;
                        let image = capture_screen(&proxy, &output.name, cursor).await?;
                        Ok(OutputCapture {
                            output: output.clone(),
                            image,
                        })
                    })
                })
            })
            .collect();
        shots
            .into_iter()
            .map(|shot| shot.join().unwrap_or_else(|_| Err(Error::Cast("screenshot thread panicked".into()))))
            .collect()
    })
}

async fn capture_screen(proxy: &ScreenShot2Proxy<'_>, name: &str, cursor: bool) -> Result<Image> {
    let (reader, writer) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|e| Error::Cast(format!("pipe: {e}")))?;
    let options = HashMap::from([
        ("native-resolution", Value::from(true)),
        ("include-cursor", Value::from(cursor)),
        // Screenie's own surfaces are concealed anyway; this keeps them out regardless.
        ("hide-caller-windows", Value::from(true)),
    ]);
    let reply = proxy.capture_screen(name, options, Fd::from(&writer)).await;
    // KWin has its own copy now: the pipe ends when it's done writing.
    drop(writer);
    let meta = reply.map_err(|e| match e {
        zbus::Error::MethodError(error, _, _) if error.as_str().ends_with("NoAuthorized") => {
            Error::Unsupported(
                "KWin doesn't let screenie take screenshots: restart screenie's daemon \
                 (`screenie quit`) so it can install its desktop file"
                    .into(),
            )
        }
        e => dbus(e),
    })?;
    let data = read_all(reader)?;
    decode(&meta, data)
}

/// The pixels KWin wrote, described by the reply to the capture.
fn decode(meta: &HashMap<String, OwnedValue>, data: Vec<u8>) -> Result<Image> {
    let number = |key: &str| -> Result<u32> {
        meta.get(key)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| Error::Cast(format!("KWin's screenshot has no {key}")))
    };
    let (width, height, stride) = (number("width")?, number("height")?, number("stride")?);
    // QImage formats, in memory on a little-endian machine.
    let format = match number("format")? {
        4 => PixelFormat::Bgrx,           // RGB32
        5 | 6 => PixelFormat::Bgra,       // ARGB32 (premultiplied)
        16 => PixelFormat::Rgbx,          // RGBX8888
        17 | 18 => PixelFormat::Rgba,     // RGBA8888 (premultiplied)
        other => {
            return Err(Error::Cast(format!(
                "KWin's screenshot is in a format screenie doesn't read ({other})"
            )));
        }
    };
    let stride = stride as usize;
    if data.len() < stride * height.saturating_sub(1) as usize + width as usize * 4 {
        return Err(Error::Cast("KWin's screenshot came short".into()));
    }
    Ok(Image::from_raw(width, height, stride, format, data))
}

/// Read the pipe to its end (KWin writes it from a thread of its own, after replying).
fn read_all(reader: OwnedFd) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    std::fs::File::from(reader)
        .read_to_end(&mut data)
        .map_err(|e| Error::Cast(format!("reading KWin's screenshot: {e}")))?;
    Ok(data)
}

fn dbus(e: zbus::Error) -> Error {
    Error::Cast(format!("KWin screenshot: {e}"))
}
