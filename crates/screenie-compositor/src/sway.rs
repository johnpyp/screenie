//! Sway (and other i3-ipc speakers such as SwayFX) via `$SWAYSOCK`.

use std::io::{Read, Write};
use std::path::PathBuf;

use screenie_core::{Rect, WindowInfo};
use serde_json::Value;

use crate::{Compositor, Error, Result, connect};

const MAGIC: &[u8; 6] = b"i3-ipc";
const GET_TREE: u32 = 4;
const GET_OUTPUTS: u32 = 3;

pub struct Sway {
    socket: PathBuf,
}

impl Sway {
    pub fn from_env() -> Option<Self> {
        let socket = PathBuf::from(std::env::var_os("SWAYSOCK")?);
        socket.exists().then_some(Self { socket })
    }

    fn request(&self, kind: u32) -> Result<Value> {
        let mut stream = connect(&self.socket)?;
        let mut msg = Vec::with_capacity(14);
        msg.extend_from_slice(MAGIC);
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        stream.write_all(&msg)?;

        let mut header = [0u8; 14];
        stream.read_exact(&mut header)?;
        if &header[..6] != MAGIC {
            return Err(Error::Reply("bad i3-ipc magic".into()));
        }
        let len = u32::from_ne_bytes(header[6..10].try_into().expect("4 bytes")) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload)?;
        Ok(serde_json::from_slice(&payload)?)
    }
}

impl Compositor for Sway {
    fn name(&self) -> &'static str {
        "sway"
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(windows_from_tree(&self.request(GET_TREE)?))
    }

    fn focused_output(&self) -> Result<Option<String>> {
        let outputs = self.request(GET_OUTPUTS)?;
        Ok(outputs
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| o["focused"].as_bool() == Some(true))
            .and_then(|o| o["name"].as_str())
            .map(String::from))
    }
}

fn rect(v: &Value) -> Rect {
    let n = |k: &str| v[k].as_f64().unwrap_or(0.0);
    Rect::new(n("x"), n("y"), n("width"), n("height"))
}

/// Visible views, topmost first: fullscreen, then floating (last listed is on top), then
/// tiled.
pub(crate) fn windows_from_tree(tree: &Value) -> Vec<WindowInfo> {
    let (mut fullscreen, mut floating, mut tiled) = (Vec::new(), Vec::new(), Vec::new());
    collect(tree, false, &mut fullscreen, &mut floating, &mut tiled);
    floating.reverse();
    fullscreen
        .into_iter()
        .chain(floating)
        .chain(tiled)
        .collect()
}

fn collect(
    node: &Value,
    in_floating: bool,
    fullscreen: &mut Vec<WindowInfo>,
    floating: &mut Vec<WindowInfo>,
    tiled: &mut Vec<WindowInfo>,
) {
    if node["name"].as_str() == Some("__i3") {
        return; // the scratchpad
    }
    let children = node["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let floaters = node["floating_nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let is_view = children.is_empty()
        && floaters.is_empty()
        && matches!(node["type"].as_str(), Some("con" | "floating_con"))
        && node.get("pid").is_some();

    if is_view {
        if node["visible"].as_bool() != Some(true) {
            return;
        }
        let outer = rect(&node["rect"]);
        let inner = rect(&node["window_rect"]);
        let content = if inner.is_empty() {
            outer
        } else {
            inner.translate(outer.x, outer.y)
        };
        let info = WindowInfo {
            id: node["id"].to_string(),
            title: node["name"].as_str().unwrap_or_default().to_string(),
            app_id: node["app_id"]
                .as_str()
                .or_else(|| node["window_properties"]["class"].as_str())
                .unwrap_or_default()
                .to_string(),
            rect: content,
            focused: node["focused"].as_bool().unwrap_or(false),
            floating: in_floating || node["type"].as_str() == Some("floating_con"),
            toplevel: node["foreign_toplevel_identifier"]
                .as_str()
                .map(String::from),
        };
        if node["fullscreen_mode"].as_u64().unwrap_or(0) > 0 {
            fullscreen.push(info);
        } else if info.floating {
            floating.push(info);
        } else {
            tiled.push(info);
        }
        return;
    }
    for child in children {
        collect(child, in_floating, fullscreen, floating, tiled);
    }
    for child in floaters {
        collect(child, true, fullscreen, floating, tiled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacking_order() {
        let tree = serde_json::json!({
            "type": "root", "name": "root", "nodes": [
                {"type": "output", "name": "__i3", "nodes": [{"type": "workspace", "nodes": [
                    {"type": "con", "pid": 9, "visible": true, "rect": {"x":0,"y":0,"width":1,"height":1}, "nodes": []}
                ]}]},
                {"type": "output", "name": "DP-1", "nodes": [{"type": "workspace", "nodes": [
                    {"type": "con", "id": 1, "pid": 1, "name": "tiled", "app_id": "a", "visible": true,
                     "rect": {"x":0,"y":0,"width":100,"height":100},
                     "window_rect": {"x":2,"y":2,"width":96,"height":96}, "nodes": [], "floating_nodes": []},
                    {"type": "con", "id": 2, "pid": 2, "name": "hidden", "visible": false,
                     "rect": {"x":0,"y":0,"width":100,"height":100}, "nodes": [], "floating_nodes": []}
                ], "floating_nodes": [
                    {"type": "floating_con", "id": 3, "pid": 3, "name": "below", "visible": true,
                     "rect": {"x":10,"y":10,"width":50,"height":50}, "nodes": [], "floating_nodes": []},
                    {"type": "floating_con", "id": 4, "pid": 4, "name": "above", "visible": true,
                     "rect": {"x":20,"y":20,"width":50,"height":50}, "nodes": [], "floating_nodes": []}
                ]}]}
            ]
        });
        let windows = windows_from_tree(&tree);
        let titles: Vec<_> = windows.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, ["above", "below", "tiled"]);
        assert_eq!(windows[2].rect, Rect::new(2.0, 2.0, 96.0, 96.0));
    }
}
