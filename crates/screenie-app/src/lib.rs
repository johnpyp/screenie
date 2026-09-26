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
mod last;
mod preview;
mod recording;
mod screenshot;
mod server;

use std::sync::Arc;

use screenie_capture::CaptureContext;
use screenie_config::{Config, Paths};

/// This session's daemon, once it has claimed the socket and before it serves: the
/// place to set up anything only the one daemon may touch (its log).
pub struct Claimed {
    listener: std::os::unix::net::UnixListener,
    claim: screenie_ipc::SocketClaim,
}

/// Become the session's daemon. Fails if another one is running.
pub fn claim() -> anyhow::Result<Claimed> {
    // However it was started, the daemon lives on its own: it holds no directory busy,
    // and nothing it does depends on one (the CLI sends absolute paths).
    std::env::set_current_dir("/")?;
    let (listener, claim) = screenie_ipc::bind_listener(&Paths::get().socket())?;
    Ok(Claimed { listener, claim })
}

impl Claimed {
    /// Run the daemon until asked to quit. `commit` describes the build (for `status`).
    pub fn run(self, commit: &'static str) -> anyhow::Result<()> {
        run(self, commit)
    }
}

fn run(claimed: Claimed, commit: &'static str) -> anyhow::Result<()> {
    // Held until `run` returns, when it removes the socket.
    let Claimed {
        listener,
        claim: _claim,
    } = claimed;
    tracing::info!(socket = %Paths::get().socket().display(), "daemon listening");
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
            cx.spawn(async move |cx| daemon::serve(incoming, cx).await)
                .detach();
            cx.spawn(async move |cx| daemon::watch_config(cx).await)
                .detach();
        });
    Ok(())
}
