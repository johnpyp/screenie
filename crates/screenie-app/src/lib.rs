//! The screenie daemon.
//!
//! One long-lived process per Wayland session owns everything that must outlive a CLI
//! invocation: the GPUI app (so overlays appear instantly), clipboard contents, the
//! preview stack, and running recordings. The `screenie` CLI starts it on demand.

mod appearance;
mod clipboard;
mod daemon;
mod deliver;
mod editor;
mod preview;
mod recording;
mod screenshot;
mod server;

use std::sync::Arc;

use screenie_capture::CaptureContext;
use screenie_config::{Config, Paths};

/// Run the daemon until asked to quit. `commit` describes the build (for `status`).
pub fn run(commit: &'static str) -> anyhow::Result<()> {
    let socket = Paths::get().socket();
    let listener = screenie_ipc::bind_listener(&socket)?;
    tracing::info!(socket = %socket.display(), "daemon listening");
    let incoming = server::start(listener);

    let config = Config::load_or_default();
    let capture = Arc::new(CaptureContext::new());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), commit, build = %screenie_ipc::exe_stamp(), "starting");

    gpui_kit::platform::application()
        .with_assets(screenie_ui_kit::Assets)
        // A daemon has no windows most of the time.
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
        screenie_ui_kit::init(cx);
        cx.set_global(daemon::Daemon::new(config, capture, commit));
        appearance::start(cx);
        cx.spawn(async move |cx| daemon::serve(incoming, cx).await).detach();
        cx.spawn(async move |cx| daemon::watch_config(cx).await).detach();
    });

    let _ = std::fs::remove_file(&socket);
    Ok(())
}
