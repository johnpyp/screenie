//! KDE Plasma's KWin, over D-Bus.
//!
//! KWin tells no client where windows are, but it runs scripts it's handed
//! (`org.kde.kwin.Scripting`), and a script can call out over D-Bus. So [`Kwin::windows`]
//! loads a small script that lists the visible windows, topmost first, and sends them
//! back to a D-Bus object this process serves; then unloads it. The focused output is a
//! plain call (`activeOutputName`). Neither is restricted.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use screenie_core::{Rect, WindowInfo};
use screenie_desktop::Desktop;
use serde::Deserialize;

use crate::{Compositor, Error, Result};

/// Where the script sends its results.
const PATH: &str = "/dev/johnpyp/Screenie/KWinScript";
const INTERFACE: &str = "dev.johnpyp.Screenie.KWinScript";
/// How long KWin gets to run the script.
const TIMEOUT: Duration = Duration::from_secs(1);

/// The window list, topmost first, as JSON: KWin 6's API, with KWin 5's where 6 renamed it.
const SCRIPT: &str = r#"
const kwin6 = typeof workspace.windowList === "function";
const all = kwin6 ? workspace.stackingOrder.slice().reverse() : workspace.clientList();
const active = kwin6 ? workspace.activeWindow : workspace.activeClient;
function onCurrentDesktop(w) {
    if (w.onAllDesktops) return true;
    if (!kwin6) return w.desktop === workspace.currentDesktop;
    const current = typeof workspace.currentDesktopForScreen === "function"
        ? workspace.currentDesktopForScreen(w.output) : workspace.currentDesktop;
    return w.desktops.some(d => d.id === current.id);
}
function onCurrentActivity(w) {
    return !w.activities || w.activities.length === 0
        || w.activities.includes(workspace.currentActivity);
}
const windows = [];
for (const w of all) {
    if (w.deleted || w.minimized || w.hidden || !(w.normalWindow || w.dialog)) continue;
    if (!onCurrentDesktop(w) || !onCurrentActivity(w)) continue;
    const g = w.frameGeometry;
    windows.push({
        id: w.internalId.toString(),
        title: w.caption,
        app_id: w.desktopFileName || w.resourceClass || "",
        x: g.x, y: g.y, width: g.width, height: g.height,
        focused: w === active,
    });
}
callDBus("{bus}", "{path}", "{interface}", "Windows", "{request}", JSON.stringify(windows));
"#;

#[zbus::proxy(
    interface = "org.kde.kwin.Scripting",
    default_service = "org.kde.KWin",
    default_path = "/Scripting",
    gen_blocking = false
)]
trait Scripting {
    #[zbus(name = "loadScript")]
    fn load_script(&self, path: &str, plugin: &str) -> zbus::Result<i32>;
    #[zbus(name = "unloadScript")]
    fn unload_script(&self, plugin: &str) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.kde.kwin.Script",
    default_service = "org.kde.KWin",
    gen_blocking = false
)]
trait Script {
    fn run(&self) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.kde.KWin",
    default_service = "org.kde.KWin",
    default_path = "/KWin",
    gen_blocking = false
)]
trait KWin {
    #[zbus(name = "activeOutputName")]
    fn active_output_name(&self) -> zbus::Result<String>;
}

/// Answers from the scripts, by request.
#[derive(Default)]
struct Answers(Mutex<HashMap<String, async_channel::Sender<String>>>);

struct Listener(Arc<Answers>);

#[zbus::interface(name = "dev.johnpyp.Screenie.KWinScript")]
impl Listener {
    #[zbus(name = "Windows")]
    async fn windows(&self, request: String, json: String) {
        let waiting = self.0.0.lock().unwrap().remove(&request);
        if let Some(tx) = waiting {
            let _ = tx.send(json).await;
        }
    }
}

pub struct Kwin {
    /// A connection serving the scripts' answers, made on first use.
    connection: OnceLock<Option<(zbus::Connection, Arc<Answers>)>>,
}

impl Kwin {
    /// On a Plasma session.
    pub fn from_env() -> Option<Self> {
        (Desktop::current() == Desktop::Kde).then(|| Kwin {
            connection: OnceLock::new(),
        })
    }

    fn connection(&self) -> Result<&(zbus::Connection, Arc<Answers>)> {
        self.connection
            .get_or_init(|| {
                let answers = Arc::new(Answers::default());
                let built = async_io::block_on(async {
                    zbus::connection::Builder::session()?
                        .serve_at(PATH, Listener(answers.clone()))?
                        .build()
                        .await
                });
                built
                    .inspect_err(|e| tracing::debug!("D-Bus for KWin scripts: {e}"))
                    .ok()
                    .map(|c| (c, answers))
            })
            .as_ref()
            .ok_or_else(|| Error::Reply("no D-Bus session to talk to KWin over".into()))
    }

    async fn run_script(&self) -> Result<String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (connection, answers) = self.connection()?;
        let request = format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let bus = connection
            .unique_name()
            .ok_or_else(|| Error::Reply("the D-Bus connection has no name".into()))?;
        let script = SCRIPT
            .replace("{bus}", bus.as_str())
            .replace("{path}", PATH)
            .replace("{interface}", INTERFACE)
            .replace("{request}", &request);
        let dir = std::env::temp_dir();
        let path = dir.join(format!("screenie-kwin-{request}.js"));
        std::fs::write(&path, script)?;
        let (tx, rx) = async_channel::bounded(1);
        answers.0.lock().unwrap().insert(request.clone(), tx);

        let plugin = format!("screenie-{request}");
        let result = async {
            let scripting = ScriptingProxy::new(connection).await.map_err(dbus)?;
            let id = scripting
                .load_script(&path.to_string_lossy(), &plugin)
                .await
                .map_err(dbus)?;
            if id < 0 {
                return Err(Error::Reply("KWin didn't load the window list script".into()));
            }
            let script = ScriptProxy::builder(connection)
                .path(format!("/Scripting/Script{id}"))
                .map_err(dbus)?
                .build()
                .await
                .map_err(dbus)?;
            script.run().await.map_err(dbus)?;
            let answer = futures_lite::future::or(async { rx.recv().await.ok() }, async {
                async_io::Timer::after(TIMEOUT).await;
                None
            })
            .await;
            let _ = scripting.unload_script(&plugin).await;
            answer.ok_or_else(|| Error::Reply("KWin's window list script didn't answer".into()))
        }
        .await;
        answers.0.lock().unwrap().remove(&request);
        let _ = std::fs::remove_file(&path);
        result
    }
}

impl Compositor for Kwin {
    fn name(&self) -> &'static str {
        "kwin"
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let json = async_io::block_on(self.run_script())?;
        parse_windows(&json)
    }

    fn focused_output(&self) -> Result<Option<String>> {
        let (connection, _) = self.connection()?;
        async_io::block_on(async {
            let name = KWinProxy::new(connection)
                .await
                .map_err(dbus)?
                .active_output_name()
                .await
                .map_err(dbus)?;
            Ok((!name.is_empty()).then_some(name))
        })
    }
}

#[derive(Deserialize)]
struct ScriptWindow {
    id: String,
    title: String,
    app_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    focused: bool,
}

fn parse_windows(json: &str) -> Result<Vec<WindowInfo>> {
    let windows: Vec<ScriptWindow> = serde_json::from_str(json)?;
    Ok(windows
        .into_iter()
        .map(|w| WindowInfo {
            id: w.id,
            title: w.title,
            app_id: w.app_id,
            rect: Rect::new(w.x, w.y, w.width, w.height),
            focused: w.focused,
            floating: true,
            toplevel: None,
        })
        .collect())
}

fn dbus(e: zbus::Error) -> Error {
    Error::Reply(format!("KWin: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_answers_become_windows() {
        let json = r#"[{"id":"{9c6d}","title":"Konsole","app_id":"org.kde.konsole","x":10,"y":20,"width":800,"height":600,"focused":true}]"#;
        let windows = parse_windows(json).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "{9c6d}");
        assert_eq!(windows[0].rect, Rect::new(10.0, 20.0, 800.0, 600.0));
        assert!(windows[0].focused);
    }
}
