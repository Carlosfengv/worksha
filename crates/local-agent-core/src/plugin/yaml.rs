//! Bounded, single-document YAML subset for plugin authoring.
use serde_json::{Map, Value};

use super::platform::MAX_DECLARATION_BYTES;

const MAX_DEPTH: usize = 24;

#[derive(Clone, Copy, Debug)]
pub struct InvalidYaml;

type Result<T> = std::result::Result<T, InvalidYaml>;

pub fn parse(source: &str) -> Result<Value> {
    if source.len() > MAX_DECLARATION_BYTES {
        return Err(InvalidYaml);
    }
    let mut lines = Vec::new();
    for line in source.lines() {
        let line = split(line, '#')?.next().unwrap_or_default().trim_end();
        if line.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let text = &line[indent..];
        if text.starts_with(['\t', '%']) || matches!(text, "---" | "...") {
            return Err(InvalidYaml);
        }
        lines.push((indent, text));
    }
    let mut lines = lines.into_iter();
    let first = lines
        .next()
        .filter(|(indent, _)| *indent == 0)
        .ok_or(InvalidYaml)?;
    let mut lines = lines.peekable();
    let value = block(first, &mut lines, 0)?;
    if lines.peek().is_some() {
        return Err(InvalidYaml);
    }
    Ok(value)
}

type Lines<'a> = std::iter::Peekable<std::vec::IntoIter<(usize, &'a str)>>;

fn list_item(line: &str) -> Option<&str> {
    if line == "-" {
        Some("")
    } else {
        line.strip_prefix("- ")
    }
}

fn block<'a>(first: (usize, &'a str), lines: &mut Lines<'a>, depth: usize) -> Result<Value> {
    if depth >= MAX_DEPTH {
        return Err(InvalidYaml);
    }
    let (indent, first_text) = first;
    let sequence = list_item(first_text).is_some();
    let mut items = Vec::new();
    let mut object = Map::new();
    let mut next = Some(first);
    while let Some((_, text)) = next {
        if sequence {
            let item = list_item(text).ok_or(InvalidYaml)?;
            if !item.starts_with(['{', '[']) && pair(item)?.is_some() {
                items.push(block((indent + 2, item), lines, depth + 1)?);
            } else {
                items.push(block_value(item, indent, lines, depth)?);
            }
        } else {
            let (key, value) = pair(text)?.ok_or(InvalidYaml)?;
            let value = block_value(value, indent, lines, depth)?;
            insert(&mut object, key, value)?;
        }
        next = lines.next_if(|(next_indent, _)| *next_indent == indent);
    }
    if sequence {
        Ok(Value::Array(items))
    } else {
        Ok(Value::Object(object))
    }
}

fn block_value<'a>(
    text: &str,
    indent: usize,
    lines: &mut Lines<'a>,
    depth: usize,
) -> Result<Value> {
    if text.is_empty() {
        match lines.next_if(|(next_indent, _)| *next_indent > indent) {
            Some(line) => block(line, lines, depth + 1),
            None => Ok(Value::Null),
        }
    } else {
        scalar(text, depth + 1)
    }
}

fn insert(object: &mut Map<String, Value>, key: &str, value: Value) -> Result<()> {
    if key.starts_with(['{', '[']) {
        return Err(InvalidYaml);
    }
    let key = scalar(key, 0)?.as_str().ok_or(InvalidYaml)?.to_owned();
    if key.is_empty() || key == "<<" || object.insert(key, value).is_some() {
        return Err(InvalidYaml);
    }
    Ok(())
}

fn pair(text: &str) -> Result<Option<(&str, &str)>> {
    let mut parts = split(text, ':')?;
    let first = parts.next().unwrap_or_default();
    Ok((first.len() < text.len()).then(|| (first.trim(), text[first.len() + 1..].trim())))
}

fn scalar(text: &str, depth: usize) -> Result<Value> {
    if depth >= MAX_DEPTH || text.is_empty() {
        return Err(InvalidYaml);
    }
    if let Some(inner) = text.strip_prefix('{') {
        let inner = inner.strip_suffix('}').ok_or(InvalidYaml)?.trim();
        let mut object = Map::new();
        if !inner.is_empty() {
            for item in split(inner, ',')? {
                let (key, value) = pair(item)?.ok_or(InvalidYaml)?;
                insert(&mut object, key, scalar(value, depth + 1)?)?;
            }
        }
        return Ok(Value::Object(object));
    }
    if let Some(inner) = text.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or(InvalidYaml)?.trim();
        return if inner.is_empty() {
            Ok(Value::Array(vec![]))
        } else {
            split(inner, ',')?
                .map(|item| scalar(item.trim(), depth + 1))
                .collect()
        };
    }
    if text.starts_with('"') {
        return serde_json::from_str::<String>(text)
            .map(Value::String)
            .map_err(|_| InvalidYaml);
    }
    if let Some(inner) = text.strip_prefix('\'') {
        let inner = inner.strip_suffix('\'').ok_or(InvalidYaml)?;
        return Ok(Value::String(inner.replace("''", "'")));
    }
    if text.starts_with(['&', '*', '!', '|', '>', '@', '`']) {
        return Err(InvalidYaml);
    }
    Ok(serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.into())))
}

// Quoted strings and nested flow collections own their internal separators.
fn split(text: &str, delimiter: char) -> Result<impl Iterator<Item = &str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut stack = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if let Some(active) = quote {
            if active == '"' && ch == '\\' {
                chars.next().ok_or(InvalidYaml)?;
            } else if ch == active {
                if active == '\'' && chars.peek().is_some_and(|(_, next)| *next == active) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '"' | '\'' if offset == 0 || text[..offset].ends_with([' ', ':', '[', '{', ',']) => {
                quote = Some(ch);
            }
            '[' => stack.push(']'),
            '{' => stack.push('}'),
            ']' | '}' if stack.pop() != Some(ch) => return Err(InvalidYaml),
            '#' if delimiter == '#' && (offset == 0 || text[..offset].ends_with(' ')) => {
                parts.push(&text[..offset]);
                return Ok(parts.into_iter());
            }
            _ if ch == delimiter && stack.is_empty() && delimiter != '#' => {
                if ch != ':' || chars.peek().is_none_or(|(_, next)| next.is_whitespace()) {
                    parts.push(&text[start..offset]);
                    start = offset + ch.len_utf8();
                }
            }
            _ => {}
        }
    }
    if quote.is_some() || !stack.is_empty() {
        return Err(InvalidYaml);
    }
    parts.push(&text[start..]);
    Ok(parts.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rules_and_rejects_ambiguous_documents() {
        let rules = "views:\n  - id: board\n    ref: ref:session\n    style: {group: 'it''s ready', items: [1, true, null]} # detail\n";
        assert_eq!(
            parse(rules).unwrap(),
            json!({
                "views": [{"id": "board", "ref": "ref:session", "style": {
                    "group": "it's ready", "items": [1, true, null]
                }}]
            })
        );
        for input in [
            "a: &source 1",
            "a: *source",
            "---\na: 1",
            "a: !type 1",
            "a: 1\na: 2",
            "a: {x: [1}}",
        ] {
            assert!(parse(input).is_err(), "{input}");
        }
    }
}
