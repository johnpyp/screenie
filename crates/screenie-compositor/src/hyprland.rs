//! Hyprland via its request socket (`.socket.sock`).

use std::path::PathBuf;

use screenie_core::{Rect, WindowInfo};
use serde_json::Value;

use crate::{Compositor, Result, request_to_eof};

pub struct Hyprland {
    socket: PathBuf,
}

impl Hyprland {
    pub fn from_env() -> Option<Self> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        // Hyprland moved its sockets from /tmp to the runtime dir in 0.40.
        let candidates = runtime
            .map(|r| r.join("hypr"))
            .into_iter()
            .chain([PathBuf::from("/tmp/hypr")])
            .map(|dir| dir.join(&signature).join(".socket.sock"));
        candidates
            .into_iter()
            .find(|p| p.exists())
            .map(|socket| Self { socket })
    }

    fn request(&self, command: &str) -> Result<Value> {
        let reply = request_to_eof(&self.socket, format!("j/{command}").as_bytes())?;
        Ok(serde_json::from_slice(&reply)?)
    }
}

impl Compositor for Hyprland {
    fn name(&self) -> &'static str {
        "hyprland"
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let monitors = self.request("monitors")?;
        let clients = self.request("clients")?;
        Ok(visible_windows(&monitors, &clients))
    }

    fn focused_output(&self) -> Result<Option<String>> {
        let monitors = self.request("monitors")?;
        Ok(monitors
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["focused"].as_bool() == Some(true))
            .and_then(|m| m["name"].as_str())
            .map(String::from))
    }
}

pub(crate) fn visible_windows(monitors: &Value, clients: &Value) -> Vec<WindowInfo> {
    let active: Vec<i64> = monitors
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| {
            [
                m["activeWorkspace"]["id"].as_i64(),
                m["specialWorkspace"]["id"].as_i64(),
            ]
        })
        .flatten()
        .filter(|&id| id != 0)
        .collect();

    let mut windows: Vec<(u8, i64, WindowInfo)> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["mapped"].as_bool() != Some(false) && c["hidden"].as_bool() != Some(true))
        .filter(|c| {
            c["pinned"].as_bool() == Some(true)
                || c["workspace"]["id"]
                    .as_i64()
                    .is_some_and(|id| active.contains(&id))
        })
        .map(|c| {
            let n = |v: &Value, i: usize| v[i].as_f64().unwrap_or(0.0);
            let rect = Rect::new(
                n(&c["at"], 0),
                n(&c["at"], 1),
                n(&c["size"], 0),
                n(&c["size"], 1),
            );
            // `fullscreen` was a bool before 0.42 and a mode integer since.
            let fullscreen = c["fullscreen"].as_bool().unwrap_or(false)
                || c["fullscreen"].as_i64().unwrap_or(0) > 0;
            let floating = c["floating"].as_bool().unwrap_or(false);
            let history = c["focusHistoryID"].as_i64().unwrap_or(i64::MAX);
            let layer = if fullscreen {
                0
            } else if floating {
                1
            } else {
                2
            };
            let info = WindowInfo {
                id: c["address"].as_str().unwrap_or_default().to_string(),
                title: c["title"].as_str().unwrap_or_default().to_string(),
                app_id: c["class"].as_str().unwrap_or_default().to_string(),
                rect,
                focused: history == 0,
                floating,
                toplevel: None,
            };
            (layer, history, info)
        })
        .collect();
    // Hyprland doesn't expose z-order; recently focused windows are raised, so focus
    // history is a good approximation within each layer.
    windows.sort_by_key(|(layer, history, _)| (*layer, *history));
    windows.into_iter().map(|(_, _, w)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_and_orders() {
        let monitors = serde_json::json!([
            {"name": "DP-1", "focused": true, "activeWorkspace": {"id": 1}, "specialWorkspace": {"id": 0}}
        ]);
        let clients = serde_json::json!([
            {"address": "0x1", "mapped": true, "hidden": false, "at": [0, 0], "size": [100, 100],
             "workspace": {"id": 1}, "floating": false, "fullscreen": 0, "focusHistoryID": 0, "title": "tiled", "class": "a"},
            {"address": "0x2", "mapped": true, "hidden": false, "at": [10, 10], "size": [50, 50],
             "workspace": {"id": 1}, "floating": true, "fullscreen": 0, "focusHistoryID": 2, "title": "float-old", "class": "b"},
            {"address": "0x3", "mapped": true, "hidden": false, "at": [20, 20], "size": [50, 50],
             "workspace": {"id": 1}, "floating": true, "fullscreen": false, "focusHistoryID": 1, "title": "float-new", "class": "c"},
            {"address": "0x4", "mapped": true, "hidden": false, "at": [0, 0], "size": [50, 50],
             "workspace": {"id": 2}, "floating": false, "fullscreen": 0, "focusHistoryID": 3, "title": "other-ws", "class": "d"}
        ]);
        let titles: Vec<_> = visible_windows(&monitors, &clients)
            .into_iter()
            .map(|w| w.title)
            .collect();
        assert_eq!(titles, ["float-new", "float-old", "tiled"]);
    }
}
