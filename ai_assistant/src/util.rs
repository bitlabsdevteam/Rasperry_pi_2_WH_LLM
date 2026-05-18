use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn token_estimate(text: &str) -> usize {
    text.split_whitespace().count().max(1)
}

pub fn json_escape(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

pub fn sql_escape(input: &str) -> String {
    input.replace('\'', "''")
}

pub fn truncate(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    let mut truncated = input
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

pub fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))
}

pub fn write_if_missing(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

pub fn append_text(path: &Path, contents: &str) -> Result<(), String> {
    let mut existing = if path.exists() {
        fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?
    } else {
        String::new()
    };
    existing.push_str(contents);
    fs::write(path, existing)
        .map_err(|error| format!("failed to append {}: {error}", path.display()))
}

pub fn read_to_string(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("failed to read {}: {error}", path.display()))
}

pub fn read_with_fallback(primary: &Path, fallback: &Path) -> Result<String, String> {
    if primary.exists() {
        return read_to_string(primary);
    }
    read_to_string(fallback)
}

pub fn parse_json_string(contents: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = contents.find(&needle)?;
    let after_key = &contents[start + needle.len()..];
    let colon = after_key.find(':')?;
    let mut chars = after_key[colon + 1..].chars().peekable();
    while matches!(chars.peek(), Some(ch) if ch.is_whitespace()) {
        chars.next();
    }
    if chars.next()? != '"' {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            let translated = match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                other => other,
            };
            value.push(translated);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(value),
            other => value.push(other),
        }
    }
    None
}

pub fn parse_json_usize(contents: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let start = contents.find(&needle)?;
    let after_key = &contents[start + needle.len()..];
    let colon = after_key.find(':')?;
    let mut digits = String::new();
    for ch in after_key[colon + 1..].chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse().ok()
}

pub fn parse_json_bool(contents: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let start = contents.find(&needle)?;
    let after_key = &contents[start + needle.len()..];
    let colon = after_key.find(':')?;
    let remainder = after_key[colon + 1..].trim_start();
    if remainder.starts_with("true") {
        Some(true)
    } else if remainder.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

pub fn parse_json_array(contents: &str, key: &str) -> Option<Vec<String>> {
    let needle = format!("\"{key}\"");
    let start = contents.find(&needle)?;
    let after_key = &contents[start + needle.len()..];
    let colon = after_key.find(':')?;
    let remainder = after_key[colon + 1..].trim_start();
    let open = remainder.find('[')?;
    let close = remainder[open + 1..].find(']')?;
    let inner = &remainder[open + 1..open + 1 + close];
    let mut items = Vec::new();
    for piece in inner.split(',') {
        let trimmed = piece.trim();
        if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
            items.push(trimmed[1..trimmed.len() - 1].to_string());
        }
    }
    Some(items)
}

pub fn path_title(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("untitled")
        .to_string()
}

pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("{prefix}-{}", now_epoch()));
    let _ = fs::create_dir_all(&directory);
    directory
}
