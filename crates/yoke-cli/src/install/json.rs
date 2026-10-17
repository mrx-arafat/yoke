//! JSON edits preserve the original bytes of every unchanged value.
use std::collections::HashSet;
use std::ops::Range;

use anyhow::{bail, ensure, Result};
use serde_json::Value;

struct Entry {
    key: Option<String>,
    start: usize,
    value: Range<usize>,
}

fn skip_space(raw: &str, position: &mut usize) {
    while raw
        .as_bytes()
        .get(*position)
        .is_some_and(u8::is_ascii_whitespace)
    {
        *position += 1;
    }
}

fn value_end(raw: &str, start: usize) -> Result<usize> {
    let mut stream = serde_json::Deserializer::from_str(&raw[start..]).into_iter::<Value>();
    stream
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("missing JSON value"))?;
    Ok(start + stream.byte_offset())
}

fn entries(raw: &str) -> Result<Vec<Entry>> {
    let object = raw.starts_with('{');
    let mut position = 1;
    let mut result = Vec::new();
    let mut keys = HashSet::new();
    loop {
        let start = position;
        skip_space(raw, &mut position);
        if matches!(raw.as_bytes().get(position), Some(b'}' | b']')) {
            break;
        }
        let key = if object {
            let end = value_end(raw, position)?;
            let key: String = serde_json::from_str(&raw[position..end])?;
            ensure!(keys.insert(key.clone()), "duplicate JSON key: {key}");
            position = end;
            skip_space(raw, &mut position);
            position += 1; // colon, already checked by serde_json
            skip_space(raw, &mut position);
            Some(key)
        } else {
            None
        };
        let end = value_end(raw, position)?;
        let value = position..end;
        let child = &raw[value.clone()];
        if child.starts_with('{') || child.starts_with('[') {
            entries(child)?; // Reject ambiguous duplicate keys, including nested ones.
        }
        result.push(Entry { key, start, value });
        position = end;
        skip_space(raw, &mut position);
        if raw.as_bytes().get(position) == Some(&b',') {
            position += 1;
        }
    }
    Ok(result)
}

pub(super) fn parse(raw: &str) -> Result<Value> {
    let parsed: Value = serde_json::from_str(raw)?;
    ensure!(parsed.is_object(), "hook settings must be a JSON object");
    entries(raw.trim())?;
    Ok(parsed)
}

/// Re-render changed containers while retaining the bytes of unchanged values.
pub(super) fn render(raw: &str, after: &Value) -> Result<String> {
    let before: Value = serde_json::from_str(raw)?;
    if before == *after {
        return Ok(raw.to_owned());
    }
    let trimmed = raw.trim();
    let prefix = raw.len() - raw.trim_start().len();
    let content = match (&before, after) {
        (Value::Object(old), Value::Object(new)) => {
            let spans = entries(trimmed)?;
            let trailing = spans
                .last()
                .map_or(&trimmed[1..trimmed.len() - 1], |entry| {
                    &trimmed[entry.value.end..trimmed.len() - 1]
                });
            let mut result = String::from("{");
            let mut count = 0;
            for entry in spans {
                let key = entry.key.as_ref().unwrap();
                if let Some(value) = new.get(key) {
                    if count > 0 {
                        result.push(',');
                    }
                    result.push_str(&trimmed[entry.start..entry.value.start]);
                    result.push_str(&render(&trimmed[entry.value], value)?);
                    count += 1;
                }
            }
            for (key, value) in new {
                if !old.contains_key(key) {
                    if count > 0 {
                        result.push(',');
                    }
                    result.push_str(&serde_json::to_string(key)?);
                    result.push(':');
                    result.push_str(&serde_json::to_string(value)?);
                    count += 1;
                }
            }
            result.push_str(trailing);
            result.push('}');
            result
        }
        (Value::Array(old), Value::Array(new)) => {
            let spans = entries(trimmed)?;
            let mut used = HashSet::new();
            let mut parts = Vec::new();
            for (new_index, value) in new.iter().enumerate() {
                if let Some((index, _)) = old
                    .iter()
                    .enumerate()
                    .find(|(i, v)| !used.contains(i) && *v == value)
                {
                    used.insert(index);
                    parts.push(trimmed[spans[index].start..spans[index].value.end].to_owned());
                } else if old.len() == new.len()
                    && !used.contains(&new_index)
                    && old[new_index].is_object()
                    && value.is_object()
                {
                    used.insert(new_index);
                    parts.push(format!(
                        "{}{}",
                        &trimmed[spans[new_index].start..spans[new_index].value.start],
                        render(&trimmed[spans[new_index].value.clone()], value)?
                    ));
                } else {
                    parts.push(serde_json::to_string(value)?);
                }
            }
            let trailing = spans
                .last()
                .map_or(&trimmed[1..trimmed.len() - 1], |entry| {
                    &trimmed[entry.value.end..trimmed.len() - 1]
                });
            format!("[{}{}]", parts.join(","), trailing)
        }
        _ => serde_json::to_string(after)?,
    };
    let result = format!(
        "{}{}{}",
        &raw[..prefix],
        content,
        &raw[prefix + trimmed.len()..]
    );
    if serde_json::from_str::<Value>(&result)? != *after {
        bail!("JSON edit did not produce the intended settings");
    }
    Ok(result)
}
