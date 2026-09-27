//! Screenie's GNOME Shell extension ([`crate::extension`]), as screenie asks it things:
//! `dev.johnpyp.Screenie.Shell` on the session bus, there while the extension runs
//! (not before the login after it's installed, nor while the screen is locked).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use screenie_core::Rect;
use zbus::Connection;
use zbus::zvariant::Value;

const NAME: &str = "dev.johnpyp.Screenie.Shell";

#[zbus::proxy(
    interface = "dev.johnpyp.Screenie.Shell",
    default_service = "dev.johnpyp.Screenie.Shell",
    default_path = "/dev/johnpyp/Screenie/Shell",
    gen_blocking = false
)]
trait Service {
    #[zbus(property)]
    fn features(&self) -> zbus::Result<Vec<String>>;
    fn windows(&self) -> zbus::Result<Vec<(u64, String, String, (i32, i32, i32, i32), bool)>>;
    fn pointer_output(&self, outputs: &[&str]) -> zbus::Result<String>;
    fn quiet_next_cast(&self) -> zbus::Result<()>;
    fn cast_started(&self) -> zbus::Result<()>;
    fn place(&self, title: &str, layer: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
}

/// What the running extension does for screenie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Lists windows ([`Shell::windows`]).
    Windows,
    /// Says where the pointer is ([`Shell::pointer_output`]).
    Pointer,
    /// Keeps screenie's casts out of the top bar ([`Shell::quiet_next_cast`]).
    QuietCasts,
    /// Makes windows that take the keyboard layer surfaces ([`place`]).
    Overlays,
    /// Makes windows that don't take it layer surfaces too (GNOME 49 on).
    Floating,
}

impl Feature {
    fn name(self) -> &'static str {
        match self {
            Feature::Windows => "windows",
            Feature::Pointer => "pointer",
            Feature::QuietCasts => "quiet-casts",
            Feature::Overlays => "overlays",
            Feature::Floating => "floating",
        }
    }
}

/// How long [`offers`] trusts what it heard: the extension comes and goes (GNOME turns
/// extensions off while the screen is locked).
const OFFERS_FOR: Duration = Duration::from_secs(2);

/// Whether the extension is running, and does `feature`. Blocking, but asks at most
/// every couple of seconds.
pub fn offers(feature: Feature) -> bool {
    static HEARD: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    let mut heard = HEARD.lock().unwrap();
    if !heard
        .as_ref()
        .is_some_and(|(at, _)| at.elapsed() < OFFERS_FOR)
    {
        let features = async_io::block_on(async {
            let shell = Shell::running(connection().await.ok()?).await?;
            shell.proxy.features().await.ok()
        });
        *heard = Some((Instant::now(), features.unwrap_or_default()));
    }
    heard
        .as_ref()
        .is_some_and(|(_, features)| features.iter().any(|f| f == feature.name()))
}

/// A layer-shell surface, as a window stands for one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer<'a> {
    /// The output's connector name.
    pub output: &'a str,
    /// The edges it's anchored to: top 1, bottom 2, left 4, right 8, as layer-shell has.
    pub anchor: u32,
    /// Top, right, bottom, left.
    pub margin: (i32, i32, i32, i32),
    /// Whether it takes the keyboard.
    pub keyboard: bool,
    /// -1 to cover the top bar, 0 to keep clear of it.
    pub exclusive: i32,
}

/// The next window of screenie's titled `title` is `layer`: the extension makes it that
/// surface as it opens (see the extension's `layers.js`). Blocking.
pub fn place(title: &str, layer: &Layer<'_>) -> zbus::Result<()> {
    async_io::block_on(async {
        let connection = connection().await?;
        let shell = Shell::running(connection).await.ok_or_else(|| {
            zbus::Error::Failure("screenie's GNOME Shell extension isn't running".into())
        })?;
        let layer = HashMap::from([
            ("output", Value::from(layer.output)),
            ("anchor", Value::from(layer.anchor)),
            ("margin", Value::from(layer.margin)),
            ("keyboard", Value::from(layer.keyboard)),
            ("exclusive", Value::from(layer.exclusive)),
        ]);
        shell.proxy.place(title, layer).await
    })
}

/// The session bus, one connection for the process's calls here.
async fn connection() -> zbus::Result<&'static Connection> {
    static CONNECTION: OnceLock<Connection> = OnceLock::new();
    if let Some(connection) = CONNECTION.get() {
        return Ok(connection);
    }
    let connection = Connection::session().await?;
    Ok(CONNECTION.get_or_init(|| connection))
}

/// A window, as the extension lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Mutter's id for it, which its screen casts take (`window-id`).
    pub id: u64,
    pub title: String,
    pub app_id: String,
    /// The visible frame, in the outputs' logical layout.
    pub rect: Rect,
    pub focused: bool,
}

/// The running extension, if it is, and if it does `feature`.
pub struct Shell {
    proxy: ServiceProxy<'static>,
}

impl Shell {
    pub async fn connect(connection: &Connection, feature: Feature) -> Option<Shell> {
        let shell = Shell::running(connection).await?;
        let features = shell
            .proxy
            .features()
            .await
            .inspect_err(|e| tracing::debug!("screenie's GNOME Shell extension: {e}"))
            .ok()?;
        features
            .iter()
            .any(|f| f == feature.name())
            .then_some(shell)
    }

    /// The extension, if it's running.
    async fn running(connection: &Connection) -> Option<Shell> {
        let running = zbus::fdo::DBusProxy::new(connection)
            .await
            .ok()?
            .name_has_owner(NAME.try_into().expect("a valid bus name"))
            .await
            .unwrap_or(false);
        if !running {
            return None;
        }
        let proxy = ServiceProxy::builder(connection)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .ok()?;
        Some(Shell { proxy })
    }

    /// The windows on the active workspace, topmost first, but not screenie's.
    pub async fn windows(&self) -> zbus::Result<Vec<Window>> {
        Ok(self
            .proxy
            .windows()
            .await?
            .into_iter()
            .map(
                |(id, title, app_id, (x, y, width, height), focused)| Window {
                    id,
                    title,
                    app_id,
                    rect: Rect::new(x as f64, y as f64, width as f64, height as f64),
                    focused,
                },
            )
            .collect())
    }

    /// Which of `outputs` (connector names) the pointer is on.
    pub async fn pointer_output(&self, outputs: &[&str]) -> zbus::Result<Option<String>> {
        let output = self.proxy.pointer_output(outputs).await?;
        Ok((!output.is_empty()).then_some(output))
    }

    /// The next screen cast screenie starts, within a couple of seconds, doesn't show in
    /// the top bar. It has to be marked a recording (`is-recording`). Say
    /// [`Shell::cast_started`] once it has, or failed to.
    pub async fn quiet_next_cast(&self) -> zbus::Result<()> {
        self.proxy.quiet_next_cast().await
    }

    pub async fn cast_started(&self) -> zbus::Result<()> {
        self.proxy.cast_started().await
    }
}
