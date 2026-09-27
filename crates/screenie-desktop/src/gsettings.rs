//! GNOME's settings, through the `gsettings` tool, which every GNOME has.

use std::process::Command;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A string setting, or a list of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    List(Vec<String>),
    One(String),
}

impl Value {
    /// As GVariant text, for `gsettings set`.
    pub fn text(&self) -> String {
        match self {
            Value::List(list) if list.is_empty() => "@as []".to_string(),
            Value::List(list) => {
                let items: Vec<String> = list.iter().map(|a| quote(a)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::One(one) => quote(one),
        }
    }

    /// Read as `gsettings get` prints it, if it's a string or a list of them.
    pub fn parse(text: &str) -> Option<Value> {
        let text = text.trim();
        if let Some(inner) = text
            .strip_prefix("@as ")
            .unwrap_or(text)
            .strip_prefix('[')
            .and_then(|t| t.strip_suffix(']'))
        {
            let mut items = Vec::new();
            let mut rest = inner.trim();
            while !rest.is_empty() {
                let (item, tail) = parse_quoted(rest)?;
                items.push(item);
                rest = tail.trim_start();
                rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
            }
            return Some(Value::List(items));
        }
        parse_string(text).map(Value::One)
    }
}

pub fn get(schema: &str, key: &str) -> Result<Option<Value>> {
    Ok(Value::parse(&run(&["get", schema, key])?))
}

/// A string setting.
pub fn get_string(schema: &str, key: &str) -> Result<Option<String>> {
    Ok(parse_string(run(&["get", schema, key])?.trim()))
}

/// A list setting, empty if it isn't one.
pub fn get_list(schema: &str, key: &str) -> Result<Vec<String>> {
    match get(schema, key)? {
        Some(Value::List(list)) => Ok(list),
        _ => Ok(Vec::new()),
    }
}

pub fn set(schema: &str, key: &str, value: &Value) -> Result<()> {
    run(&["set", schema, key, &value.text()]).map(drop)
}

pub fn run(args: &[&str]) -> Result<String> {
    let output = Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|e| Error(format!("running gsettings: {e}")))?;
    if !output.status.success() {
        return Err(Error(format!(
            "gsettings {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_string(text: &str) -> Option<String> {
    match parse_quoted(text)? {
        (s, "") => Some(s),
        _ => None,
    }
}

/// A quoted string at the start of `text`, and what follows it.
fn parse_quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices();
    let (_, open) = chars.next().filter(|(_, c)| *c == '\'' || *c == '"')?;
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                c => out.push(c),
            },
            c if c == open => return Some((out, &text[i + 1..])),
            c => out.push(c),
        }
    }
    None
}

/// `text` as a GVariant string.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_gsettings_prints_them() {
        assert_eq!(Value::parse("@as []"), Some(Value::List(vec![])));
        assert_eq!(
            Value::parse("['<Shift>Print', '<Super>p']"),
            Some(Value::List(vec!["<Shift>Print".into(), "<Super>p".into()]))
        );
        assert_eq!(Value::parse("'Print'"), Some(Value::One("Print".into())));
        assert_eq!(Value::parse("\"it's\""), Some(Value::One("it's".into())));
        assert_eq!(Value::parse("true"), None);
        assert_eq!(Value::parse("uint32 5"), None);
        let tricky = r"/opt/my 'tools'/screenie \ shot";
        assert_eq!(parse_string(&quote(tricky)).as_deref(), Some(tricky));
        let list = Value::List(vec![tricky.into(), "b".into()]);
        assert_eq!(Value::parse(&list.text()), Some(list));
    }
}
