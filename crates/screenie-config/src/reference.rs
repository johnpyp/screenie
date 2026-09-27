//! The config's reference, made from the schema's doc comments: every key, with what it
//! does, its default and the words it takes. `screenie(5)` is rendered from it.

use serde_json::{Map, Value};

use crate::Config;

/// One key of the config file.
#[derive(Debug)]
pub struct Key {
    /// Where it sits, like `recording.framerate`.
    pub path: String,
    /// What it does, in paragraphs, with `code` in backticks: the field's doc comment,
    /// then its type's.
    pub description: Vec<String>,
    /// Its default as written in the file (`true`, `high`, `"#ff3b30"`). None for a
    /// group of keys.
    pub default: Option<String>,
    /// The words it takes, each with what it means where that's documented.
    pub values: Vec<(String, Option<String>)>,
}

impl Key {
    /// A group of keys, like `screenshot` or `screenshot.after_capture`.
    pub fn is_group(&self) -> bool {
        self.default.is_none()
    }
}

/// Every key, in the file's order, each group followed by its keys.
pub fn keys() -> Vec<Key> {
    let schema = schemars::schema_for!(Config);
    let root = schema.as_value();
    let defs = root.get("$defs").and_then(Value::as_object);
    let mut keys = Vec::new();
    walk(root, defs, "", &mut keys);
    keys
}

/// A short config, for the reference to show how it's written.
pub const EXAMPLE: &str = "\
screenshot:
  directory: ~/Pictures/Screenshots
  after_capture: { copy: true, preview: false }
recording:
  framerate: 30
  microphone: true
";

fn walk(schema: &Value, defs: Option<&Map<String, Value>>, prefix: &str, keys: &mut Vec<Key>) {
    let properties = resolve(schema, defs).get("properties");
    for (name, property) in properties.and_then(Value::as_object).into_iter().flatten() {
        let path = match prefix {
            "" => name.clone(),
            _ => format!("{prefix}.{name}"),
        };
        let target = resolve(property, defs);
        let group = target.get("properties").is_some();
        let mut description = paragraphs(property);
        if !std::ptr::eq(target, property) {
            description.extend(paragraphs(target));
        }
        keys.push(Key {
            path: path.clone(),
            description,
            default: property.get("default").filter(|_| !group).map(written),
            values: values(target),
        });
        if group {
            walk(target, defs, &path, keys);
        }
    }
}

/// What a `$ref` points to, or the schema itself.
fn resolve<'a>(schema: &'a Value, defs: Option<&'a Map<String, Value>>) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
        .and_then(|name| defs?.get(name))
        .unwrap_or(schema)
}

/// An `enum` of words, or a `oneOf` of documented ones.
fn values(schema: &Value) -> Vec<(String, Option<String>)> {
    if let Some(words) = schema.get("enum").and_then(Value::as_array) {
        return words
            .iter()
            .filter_map(|w| Some((w.as_str()?.to_owned(), None)))
            .collect();
    }
    let one_of = schema.get("oneOf").and_then(Value::as_array);
    one_of
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let word = v.get("const")?.as_str()?.to_owned();
            Some((
                word,
                Some(paragraphs(v).join(" ")).filter(|d| !d.is_empty()),
            ))
        })
        .collect()
}

/// A schema's description, its doc comment's lines joined into paragraphs.
fn paragraphs(schema: &Value) -> Vec<String> {
    let doc = schema.get("description").and_then(Value::as_str);
    doc.into_iter()
        .flat_map(|doc| doc.split("\n\n"))
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

/// A default as it's written in the file: a word bare, other text quoted, and a list in
/// YAML's flow style.
fn written(value: &Value) -> String {
    match value {
        Value::String(s)
            if !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) =>
        {
            s.clone()
        }
        Value::Array(items) => {
            let items: Vec<_> = items.iter().map(written).collect();
            format!("[{}]", items.join(", "))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_documented() {
        let keys = keys();
        let undocumented: Vec<_> = keys
            .iter()
            .filter(|k| k.description.is_empty())
            .map(|k| &k.path)
            .collect();
        assert!(
            undocumented.is_empty(),
            "document {undocumented:?} in schema.rs"
        );
        assert!(
            keys.iter()
                .all(|k| k.values.iter().all(|(w, _)| !w.is_empty()))
        );
    }

    #[test]
    fn keys_come_in_the_files_order_with_their_defaults() {
        let keys = keys();
        let key = |path: &str| keys.iter().find(|k| k.path == path).unwrap();
        assert_eq!(keys[0].path, "ui_scale");
        assert_eq!(key("ui_scale").default.as_deref(), Some("auto"));
        assert!(key("screenshot.after_capture").is_group());
        assert_eq!(
            key("screenshot.after_capture.preview").default.as_deref(),
            Some("true")
        );
        assert_eq!(key("recording.framerate").default.as_deref(), Some("60"));
        assert_eq!(
            key("editor.default_color").default.as_deref(),
            Some("\"#ff3b30\"")
        );
        assert!(
            key("editor.palette")
                .default
                .as_ref()
                .unwrap()
                .starts_with("[\"#ff3b30\", ")
        );
        assert_eq!(key("recording.quality").values.len(), 4);
        let backends = &key("advanced.capture_backend").values;
        assert!(backends.iter().all(|(_, doc)| doc.is_some()));
    }

    #[test]
    fn the_example_parses() {
        crate::parse(EXAMPLE).unwrap();
    }
}
