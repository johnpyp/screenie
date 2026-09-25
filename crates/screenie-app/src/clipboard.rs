//! Putting captures on the clipboard.
//!
//! Wayland clipboards are served by the process that set them, which is why the daemon
//! owns copies rather than the short-lived CLI. We use the data-control protocol (ext or
//! wlr), which works without keyboard focus.

use std::path::Path;

use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

/// What to offer. Images are offered both as pixels and, when saved, as a file, so pasting
/// into an image editor and into a file manager both do the right thing.
pub(crate) enum Content<'a> {
    Image { png: Vec<u8>, file: Option<&'a Path> },
    File(&'a Path),
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).map(|u| u.to_string()).unwrap_or_else(|_| format!("file://{}", path.display()))
}

fn file_sources(path: &Path) -> Vec<MimeSource> {
    let uri = file_uri(path);
    vec![
        MimeSource {
            source: Source::Bytes(format!("{uri}\r\n").into_bytes().into()),
            mime_type: MimeType::Specific("text/uri-list".into()),
        },
        // GNOME Files / Nautilus-compatible file managers.
        MimeSource {
            source: Source::Bytes(format!("copy\n{uri}").into_bytes().into()),
            mime_type: MimeType::Specific("x-special/gnome-copied-files".into()),
        },
    ]
}

/// Set the clipboard. Blocking only briefly: serving happens on a background thread.
pub(crate) fn copy(content: Content<'_>) -> anyhow::Result<()> {
    let sources = match content {
        Content::Image { png, file } => {
            let mut sources = vec![MimeSource {
                source: Source::Bytes(png.into_boxed_slice()),
                mime_type: MimeType::Specific("image/png".into()),
            }];
            if let Some(path) = file {
                sources.extend(file_sources(path));
            }
            sources
        }
        Content::File(path) => file_sources(path),
    };
    let mut options = Options::new();
    // Only real text should paste into text fields, not a file URI.
    options.omit_additional_text_mime_types(true);
    options.copy_multi(sources).map_err(|e| anyhow::anyhow!("clipboard: {e}"))
}
