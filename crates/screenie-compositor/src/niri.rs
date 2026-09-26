//! niri via `$NIRI_SOCKET` (JSON lines).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use screenie_core::{Rect, WindowInfo};
use serde_json::Value;

use crate::{Compositor, Error, Result, connect};

pub struct Niri {
    socket: PathBuf,
}

impl Niri {
    pub fn from_env() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("NIRI_SOCKET")?);
        socket.exists().then_some(Self { socket })
    }

    /// Send a unit request like `"Windows"` and unwrap `{"Ok": {"Windows": ...}}`.
    fn request(&self, name: &str) -> Result<Value> {
        let mut stream = connect(&self.socket)?;
        stream.write_all(format!("\"{name}\"\n").as_bytes())?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let mut reply: Value = serde_json::from_str(&line)?;
        match reply.get_mut("Ok").and_then(|ok| ok.get_mut(name)) {
            Some(v) => Ok(v.take()),
            None => Err(Error::Reply(format!(
                "niri {name}: {}",
                reply.get("Err").unwrap_or(&reply)
            ))),
        }
    }
}

impl Compositor for Niri {
    fn name(&self) -> &'static str {
        "niri"
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let windows = self.request("Windows")?;
        let workspaces = self.request("Workspaces")?;
        let outputs = self.request("Outputs")?;
        Ok(visible_windows(&windows, &workspaces, &outputs))
    }

    fn focused_output(&self) -> Result<Option<String>> {
        let output = self.request("FocusedOutput")?;
        Ok(output["name"].as_str().map(String::from))
    }
}

pub(crate) fn visible_windows(
    windows: &Value,
    workspaces: &Value,
    outputs: &Value,
) -> Vec<WindowInfo> {
    // Active workspace id -> its output's rect in the global layout.
    let output_of = |workspace_id: u64| -> Option<Rect> {
        let ws = workspaces.as_array()?.iter().find(|w| {
            w["id"].as_u64() == Some(workspace_id) && w["is_active"].as_bool() == Some(true)
        })?;
        let logical = &outputs[ws["output"].as_str()?]["logical"];
        Some(Rect::new(
            logical["x"].as_f64()?,
            logical["y"].as_f64()?,
            logical["width"].as_f64()?,
            logical["height"].as_f64()?,
        ))
    };

    // Floating windows above tiled ones, then most recently focused first.
    type Stacking = (bool, std::cmp::Reverse<(u64, u64)>);
    let mut found: Vec<(Stacking, WindowInfo)> = windows
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let output = output_of(w["workspace_id"].as_u64()?)?;
            let layout = &w["layout"];
            // Tiles scrolled out of view have no position.
            let tile = &layout["tile_pos_in_workspace_view"];
            let (tx, ty) = (tile[0].as_f64()?, tile[1].as_f64()?);
            let off = &layout["window_offset_in_tile"];
            let (dx, dy) = (
                off[0].as_f64().unwrap_or(0.0),
                off[1].as_f64().unwrap_or(0.0),
            );
            let size = &layout["window_size"];
            let rect = Rect::new(
                output.x + tx + dx,
                output.y + ty + dy,
                size[0].as_f64()?,
                size[1].as_f64()?,
            );
            // niri draws a window only on its workspace's output: a column scrolled
            // partly past the edge is cut off there, not continued on the next monitor.
            let rect = rect.intersection(&output)?;
            let floating = w["is_floating"].as_bool().unwrap_or(false);
            let focused = w["is_focused"].as_bool().unwrap_or(false);
            let ts = &w["focus_timestamp"];
            let recency = (
                ts["secs"].as_u64().unwrap_or(0),
                ts["nanos"].as_u64().unwrap_or(0),
            );
            let info = WindowInfo {
                id: w["id"].to_string(),
                title: w["title"].as_str().unwrap_or_default().to_string(),
                app_id: w["app_id"].as_str().unwrap_or_default().to_string(),
                rect,
                focused,
                floating,
                toplevel: None,
            };
            Some(((!floating, std::cmp::Reverse(recency)), info))
        })
        .collect();
    found.sort_by_key(|(stacking, _)| *stacking);
    found.into_iter().map(|(_, w)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_global() {
        let outputs = serde_json::json!({"DP-2": {"logical": {"x": 1920, "y": 0, "width": 1280, "height": 720, "scale": 1.5}}});
        let workspaces = serde_json::json!([
            {"id": 5, "output": "DP-2", "is_active": true},
            {"id": 6, "output": "DP-2", "is_active": false}
        ]);
        let windows = serde_json::json!([
            {"id": 1, "title": "t", "app_id": "a", "workspace_id": 5, "is_floating": false, "is_focused": true,
             "layout": {"tile_pos_in_workspace_view": [16.0, 16.0], "window_offset_in_tile": [4.0, 4.0], "window_size": [600, 400]}},
            {"id": 2, "title": "offscreen", "workspace_id": 5, "is_floating": false,
             "layout": {"tile_pos_in_workspace_view": null, "window_offset_in_tile": [0.0, 0.0], "window_size": [600, 400]}},
            {"id": 3, "title": "inactive", "workspace_id": 6, "is_floating": false,
             "layout": {"tile_pos_in_workspace_view": [0.0, 0.0], "window_offset_in_tile": [0.0, 0.0], "window_size": [600, 400]}}
        ]);
        let found = visible_windows(&windows, &workspaces, &outputs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rect, Rect::new(1940.0, 20.0, 600.0, 400.0));
    }

    #[test]
    fn windows_are_cut_off_at_their_output() {
        let outputs = serde_json::json!({
            "DP-1": {"logical": {"x": 0, "y": 0, "width": 1920, "height": 1080, "scale": 1.0}},
            "DP-2": {"logical": {"x": 1920, "y": 0, "width": 1920, "height": 1080, "scale": 1.0}}
        });
        let workspaces = serde_json::json!([{"id": 1, "output": "DP-1", "is_active": true}]);
        let tile = |id: u64, x: f64| {
            serde_json::json!({"id": id, "title": "t", "workspace_id": 1, "is_floating": false,
             "layout": {"tile_pos_in_workspace_view": [x, 0.0], "window_offset_in_tile": [0.0, 0.0], "window_size": [600, 400]}})
        };
        // Half scrolled past the right edge, and entirely past it.
        let windows = serde_json::json!([tile(1, 1820.0), tile(2, 1960.0)]);
        let found = visible_windows(&windows, &workspaces, &outputs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rect, Rect::new(1820.0, 0.0, 100.0, 400.0));
    }
}
