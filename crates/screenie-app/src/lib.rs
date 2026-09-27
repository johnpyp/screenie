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
mod shortcuts;

use std::sync::Arc;

use screenie_capture::{CaptureContext, RestoreTokens};
use screenie_config::{Config, Paths};
use screenie_desktop::Desktop;
use screenie_state::StateFile;

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
    let state = Arc::new(StateFile::open());
    let desktop = Desktop::current();
    // KWin and the portals know screenie by its desktop entry, which has to be there
    // before capturing asks them anything: on GNOME and KDE always, elsewhere when
    // capturing goes through the portal.
    let identified = desktop != Desktop::Other && install_desktop_entry(&state);
    let capture = CaptureContext::new().remembering(Arc::new(PortalTokens(state.clone())));
    if !identified && !capture.offers().wayland.native_capture() && capture.offers().portal {
        install_desktop_entry(&state);
    }
    let capture = Arc::new(capture);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), commit, build = %screenie_ipc::exe_stamp(), desktop = desktop.name(), "starting");

    gpui_kit::platform::application()
        .with_assets(screenie_ui_kit::Assets)
        // A daemon has no windows most of the time.
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
            screenie_ui_kit::init(cx);
            cx.set_global(daemon::Daemon::new(config, capture, state, commit));
            appearance::start(cx);
            cx.spawn(async move |cx| daemon::serve(incoming, cx).await)
                .detach();
            cx.spawn(async move |cx| daemon::watch_config(cx).await)
                .detach();
        });
    Ok(())
}

/// Make sure screenie's desktop entry names this binary. Whether it's there.
fn install_desktop_entry(state: &StateFile) -> bool {
    use screenie_desktop::{entry, shortcuts};
    // KDE's keys run the entry's actions, whichever desktop this is: a home can be
    // shared by both.
    let kde = state.state().shortcuts.kde;
    let result = match &kde {
        Some(keys) => entry::ensure(&shortcuts::entry_actions(Desktop::Kde), Some(&keys.command)),
        None => entry::ensure(&entry::default_actions(), None),
    };
    result
        .inspect_err(|e| tracing::warn!("installing the desktop entry: {e}"))
        .is_ok()
}

/// The ScreenCast portal's restore token, kept in the state file.
struct PortalTokens(Arc<StateFile>);

impl RestoreTokens for PortalTokens {
    fn load(&self) -> Option<String> {
        self.0.state().portal.screencast_token
    }

    fn save(&self, token: Option<String>) {
        if let Err(e) = self.0.update(|s| s.portal.screencast_token = token) {
            tracing::warn!("{e}");
        }
    }
}
