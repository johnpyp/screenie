//! Putting captures on the clipboard.
//!
//! Wayland clipboards are served by the process that set them, which is why the daemon
//! owns copies rather than the short-lived CLI. We use the data-control protocol (ext or
//! wlr), which works without keyboard focus, and GNOME's own clipboard API where there's
//! none (see [`screenie_desktop::clipboard`]).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use screenie_desktop::clipboard::MutterClipboard;
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

/// What to offer. Images are offered both as pixels and, when saved, as a file, so pasting
/// into an image editor and into a file manager both do the right thing.
pub(crate) enum Content<'a> {
    Image {
        png: Vec<u8>,
        file: Option<&'a Path>,
    },
    File(&'a Path),
}

impl Content<'_> {
    /// Each MIME type on offer, with its bytes.
    fn offer(self) -> Vec<(String, Arc<[u8]>)> {
        let file = |path: &Path| {
            let uri = url::Url::from_file_path(path)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| format!("file://{}", path.display()));
            [
                ("text/uri-list".to_string(), format!("{uri}\r\n")),
                // GNOME Files / Nautilus-compatible file managers.
                (
                    "x-special/gnome-copied-files".to_string(),
                    format!("copy\n{uri}"),
                ),
            ]
            .map(|(mime, text)| (mime, Arc::from(text.into_bytes())))
        };
        match self {
            Content::Image { png, file: path } => {
                let mut offer = vec![("image/png".to_string(), Arc::from(png))];
                offer.extend(path.into_iter().flat_map(file));
                offer
            }
            Content::File(path) => file(path).into(),
        }
    }
}

/// Counts our copies, so something copied earlier can tell it's no longer on the
/// clipboard once we've copied something else. (Copies by other apps go unnoticed.)
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The current copy's generation (see [`copy`]).
pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// GNOME's clipboard, once it's been needed.
static MUTTER: Mutex<Option<MutterClipboard>> = Mutex::new(None);

/// Set the clipboard. Blocking only briefly: serving happens on a background thread.
pub(crate) fn copy(content: Content<'_>) -> anyhow::Result<()> {
    GENERATION.fetch_add(1, Ordering::Relaxed);
    let offer = content.offer();
    let sources = offer
        .iter()
        .map(|(mime, bytes)| MimeSource {
            source: Source::Bytes(bytes.to_vec().into_boxed_slice()),
            mime_type: MimeType::Specific(mime.clone()),
        })
        .collect();
    let mut options = Options::new();
    // Only real text should paste into text fields, not a file URI.
    options.omit_additional_text_mime_types(true);
    match options.copy_multi(sources) {
        Err(wl_clipboard_rs::copy::Error::MissingProtocol { .. }) => copy_through_mutter(offer),
        result => result.map_err(|e| anyhow::anyhow!("clipboard: {e}")),
    }
}

/// Copy with GNOME's clipboard API, connecting (again) if need be.
fn copy_through_mutter(offer: Vec<(String, Arc<[u8]>)>) -> anyhow::Result<()> {
    let mut mutter = MUTTER.lock().unwrap();
    if let Some(clipboard) = mutter.as_ref() {
        match clipboard.set(offer.clone()) {
            Ok(()) => return Ok(()),
            // The session's gone (GNOME Shell restarted, say): start another.
            Err(e) => tracing::debug!("GNOME's clipboard: {e}; reconnecting"),
        }
    }
    let clipboard = MutterClipboard::connect().map_err(|e| {
        anyhow::anyhow!(
            "clipboard: the compositor has neither data-control nor GNOME's clipboard API ({e})"
        )
    })?;
    clipboard
        .set(offer)
        .map_err(|e| anyhow::anyhow!("clipboard: {e}"))?;
    *mutter = Some(clipboard);
    Ok(())
}
