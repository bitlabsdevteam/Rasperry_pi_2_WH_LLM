use std::process::Command;

use crate::util::{json_escape, truncate};

#[derive(Clone, Debug)]
pub struct TelegramAdapter {
    bot_token: String,
    api_base_url: String,
    timeout_secs: usize,
}

#[derive(Clone, Debug)]
pub struct TelegramBot {
    pub id: i64,
    pub username: String,
    pub first_name: String,
}

#[derive(Clone, Debug)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub user_id: i64,
    pub chat_id: i64,
    pub chat_type: String,
    pub username: String,
    pub first_name: String,
    pub text: Option<String>,
}

impl TelegramUpdate {
    pub fn is_private_text(&self) -> bool {
        self.chat_type == "private"
            && self
                .text
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn display_name(&self) -> String {
        if !self.username.is_empty() {
            format!("@{}", self.username)
        } else if !self.first_name.is_empty() {
            self.first_name.clone()
        } else {
            self.user_id.to_string()
        }
    }
}

impl TelegramAdapter {
    pub fn new(bot_token: String, api_base_url: String, timeout_secs: usize) -> Self {
        Self {
            bot_token,
            api_base_url,
            timeout_secs,
        }
    }

    pub fn get_me(&self) -> Result<TelegramBot, String> {
        let response = self.call_api("getMe", None, self.timeout_secs)?;
        let value = parse_json(&response)?;
        let result = expect_object(expect_ok(&value)?)?;
        Ok(TelegramBot {
            id: get_i64(result, "id")?,
            username: get_string(result, "username").unwrap_or_default(),
            first_name: get_string(result, "first_name").unwrap_or_default(),
        })
    }

    pub fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_secs: usize,
    ) -> Result<Vec<TelegramUpdate>, String> {
        let payload = format!(
            "{{\"offset\":{},\"timeout\":{},\"allowed_updates\":[\"message\"]}}",
            offset.unwrap_or(0),
            timeout_secs
        );
        let response = self.call_api("getUpdates", Some(&payload), timeout_secs + 5)?;
        parse_updates(&response)
    }

    pub fn send_message(&self, chat_id: i64, text: &str) -> Result<(), String> {
        let payload = format!(
            "{{\"chat_id\":{},\"text\":\"{}\"}}",
            chat_id,
            json_escape(text)
        );
        let response = self.call_api("sendMessage", Some(&payload), self.timeout_secs)?;
        let value = parse_json(&response)?;
        let _ = expect_ok(&value)?;
        Ok(())
    }

    pub fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<(), String> {
        let payload = format!(
            "{{\"chat_id\":{},\"action\":\"{}\"}}",
            chat_id,
            json_escape(action)
        );
        let response = self.call_api("sendChatAction", Some(&payload), self.timeout_secs)?;
        let value = parse_json(&response)?;
        let _ = expect_ok(&value)?;
        Ok(())
    }

    fn call_api(
        &self,
        method: &str,
        payload: Option<&str>,
        max_time_secs: usize,
    ) -> Result<String, String> {
        let url = format!(
            "{}/bot{}/{}",
            self.api_base_url.trim_end_matches('/'),
            self.bot_token,
            method
        );
        let mut command = Command::new("curl");
        command.args(["-sS", "--max-time", &max_time_secs.to_string(), &url]);
        if let Some(payload) = payload {
            command.args(["-H", "content-type: application/json", "-d", payload]);
        }
        let output = command
            .output()
            .map_err(|error| format!("failed to invoke curl for Telegram {method}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Telegram {method} failed: {}",
                truncate(&String::from_utf8_lossy(&output.stderr), 240)
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

pub fn parse_updates(response: &str) -> Result<Vec<TelegramUpdate>, String> {
    let value = parse_json(response)?;
    let result = expect_ok(&value)?;
    let array = match result {
        JsonValue::Array(items) => items,
        _ => return Err("Telegram getUpdates response did not contain an array".to_string()),
    };

    let mut updates = Vec::new();
    for item in array {
        let object = match item {
            JsonValue::Object(fields) => fields,
            _ => continue,
        };
        let update_id = get_i64(&object, "update_id")?;
        let message = match get_value(&object, "message") {
            Some(JsonValue::Object(fields)) => fields,
            _ => continue,
        };
        let chat = match get_value(message, "chat") {
            Some(JsonValue::Object(fields)) => fields,
            _ => continue,
        };
        let from = match get_value(message, "from") {
            Some(JsonValue::Object(fields)) => fields,
            _ => continue,
        };
        updates.push(TelegramUpdate {
            update_id,
            user_id: get_i64(from, "id")?,
            chat_id: get_i64(chat, "id")?,
            chat_type: get_string(chat, "type").unwrap_or_default(),
            username: get_string(from, "username").unwrap_or_default(),
            first_name: get_string(from, "first_name").unwrap_or_default(),
            text: get_string(message, "text"),
        });
    }
    Ok(updates)
}

fn expect_ok<'a>(value: &'a JsonValue) -> Result<&'a JsonValue, String> {
    let object = expect_object(value)?;
    match get_value(object, "ok") {
        Some(JsonValue::Bool(true)) => get_value(object, "result")
            .ok_or_else(|| "Telegram response missing result".to_string()),
        Some(JsonValue::Bool(false)) => Err(get_string(object, "description")
            .unwrap_or_else(|| "Telegram API returned ok=false".to_string())),
        _ => Err("Telegram response missing ok field".to_string()),
    }
}

fn expect_object(value: &JsonValue) -> Result<&Vec<(String, JsonValue)>, String> {
    match value {
        JsonValue::Object(fields) => Ok(fields),
        _ => Err("expected JSON object".to_string()),
    }
}

fn get_value<'a>(fields: &'a [(String, JsonValue)], key: &str) -> Option<&'a JsonValue> {
    fields
        .iter()
        .find_map(|(name, value)| if name == key { Some(value) } else { None })
}

fn get_string(fields: &[(String, JsonValue)], key: &str) -> Option<String> {
    match get_value(fields, key) {
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn get_i64(fields: &[(String, JsonValue)], key: &str) -> Result<i64, String> {
    match get_value(fields, key) {
        Some(JsonValue::Number(value)) => Ok(*value),
        _ => Err(format!("missing Telegram numeric field `{key}`")),
    }
}

#[derive(Clone, Debug)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

struct JsonParser {
    chars: Vec<char>,
    index: usize,
}

fn parse_json(input: &str) -> Result<JsonValue, String> {
    let mut parser = JsonParser {
        chars: input.chars().collect(),
        index: 0,
    };
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.index != parser.chars.len() {
        return Err("unexpected trailing characters in JSON".to_string());
    }
    Ok(value)
}

impl JsonParser {
    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_whitespace();
        match self.peek() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('t') | Some('f') => self.parse_bool().map(JsonValue::Bool),
            Some('n') => {
                self.expect("null")?;
                Ok(JsonValue::Null)
            }
            Some('-') | Some('0'..='9') => self.parse_number().map(JsonValue::Number),
            Some(other) => Err(format!("unexpected JSON character `{other}`")),
            None => Err("unexpected end of JSON input".to_string()),
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.consume('{')?;
        let mut fields = Vec::new();
        loop {
            self.skip_whitespace();
            if self.try_consume('}') {
                break;
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.consume(':')?;
            let value = self.parse_value()?;
            fields.push((key, value));
            self.skip_whitespace();
            if self.try_consume('}') {
                break;
            }
            self.consume(',')?;
        }
        Ok(JsonValue::Object(fields))
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.consume('[')?;
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            if self.try_consume(']') {
                break;
            }
            items.push(self.parse_value()?);
            self.skip_whitespace();
            if self.try_consume(']') {
                break;
            }
            self.consume(',')?;
        }
        Ok(JsonValue::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.consume('"')?;
        let mut value = String::new();
        while let Some(ch) = self.next() {
            match ch {
                '"' => return Ok(value),
                '\\' => {
                    let escaped = self
                        .next()
                        .ok_or_else(|| "unterminated JSON escape".to_string())?;
                    match escaped {
                        '"' => value.push('"'),
                        '\\' => value.push('\\'),
                        '/' => value.push('/'),
                        'b' => value.push('\u{0008}'),
                        'f' => value.push('\u{000C}'),
                        'n' => value.push('\n'),
                        'r' => value.push('\r'),
                        't' => value.push('\t'),
                        'u' => value.push_str(&self.parse_unicode_escape()?),
                        other => return Err(format!("unsupported JSON escape `{other}`")),
                    }
                }
                other => value.push(other),
            }
        }
        Err("unterminated JSON string".to_string())
    }

    fn parse_unicode_escape(&mut self) -> Result<String, String> {
        let code = self.parse_hex_escape()?;
        if !(0xD800..=0xDFFF).contains(&code) {
            return char::from_u32(code)
                .map(|value| value.to_string())
                .ok_or_else(|| "invalid unicode scalar".to_string());
        }

        if !(0xD800..=0xDBFF).contains(&code) {
            return Err("unexpected low surrogate in unicode escape".to_string());
        }

        self.consume('\\')?;
        self.consume('u')?;
        let low = self.parse_hex_escape()?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return Err("invalid low surrogate in unicode escape".to_string());
        }

        let scalar = 0x10000 + (((code - 0xD800) << 10) | (low - 0xDC00));
        char::from_u32(scalar)
            .map(|value| value.to_string())
            .ok_or_else(|| "invalid unicode scalar".to_string())
    }

    fn parse_hex_escape(&mut self) -> Result<u32, String> {
        let mut value = String::new();
        for _ in 0..4 {
            value.push(
                self.next()
                    .ok_or_else(|| "truncated unicode escape".to_string())?,
            );
        }
        u32::from_str_radix(&value, 16).map_err(|_| "invalid unicode escape".to_string())
    }

    fn parse_bool(&mut self) -> Result<bool, String> {
        if self.remaining_starts_with("true") {
            self.expect("true")?;
            Ok(true)
        } else {
            self.expect("false")?;
            Ok(false)
        }
    }

    fn parse_number(&mut self) -> Result<i64, String> {
        let start = self.index;
        if self.try_consume('-') {}
        while matches!(self.peek(), Some('0'..='9')) {
            self.index += 1;
        }
        self.chars[start..self.index]
            .iter()
            .collect::<String>()
            .parse::<i64>()
            .map_err(|_| "invalid JSON number".to_string())
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
            self.index += 1;
        }
    }

    fn expect(&mut self, literal: &str) -> Result<(), String> {
        for expected in literal.chars() {
            let actual = self.next().ok_or_else(|| format!("expected `{literal}`"))?;
            if actual != expected {
                return Err(format!("expected `{literal}`"));
            }
        }
        Ok(())
    }

    fn consume(&mut self, expected: char) -> Result<(), String> {
        match self.next() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(format!("expected `{expected}`, found `{actual}`")),
            None => Err(format!("expected `{expected}`")),
        }
    }

    fn try_consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn remaining_starts_with(&self, literal: &str) -> bool {
        self.chars[self.index..]
            .iter()
            .zip(literal.chars())
            .all(|(left, right)| *left == right)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn next(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.index += 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonValue, parse_json, parse_updates};

    #[test]
    fn parses_private_text_updates_and_ignores_unsupported_payloads() {
        let response = r#"{
          "ok": true,
          "result": [
            {
              "update_id": 11,
              "message": {
                "message_id": 1,
                "from": {"id": 42, "first_name": "David", "username": "dbong"},
                "chat": {"id": 42, "type": "private"},
                "date": 1710000000,
                "text": "hello there"
              }
            },
            {
              "update_id": 12,
              "message": {
                "message_id": 2,
                "from": {"id": 77, "first_name": "Groupy"},
                "chat": {"id": -10, "type": "group"},
                "date": 1710000001
              }
            }
          ]
        }"#;

        let updates = parse_updates(response).unwrap();
        assert_eq!(updates.len(), 2);
        assert!(updates[0].is_private_text());
        assert_eq!(updates[0].text.as_deref(), Some("hello there"));
        assert!(!updates[1].is_private_text());
    }

    #[test]
    fn parses_surrogate_pair_emoji_in_json_strings() {
        let parsed = parse_json(r#"{"ok":true,"result":{"text":"Hello \ud83d\ude03"}}"#).unwrap();
        let JsonValue::Object(fields) = parsed else {
            panic!("expected object");
        };
        let Some(JsonValue::Object(result)) =
            fields.iter().find_map(
                |(key, value)| {
                    if key == "result" { Some(value) } else { None }
                },
            )
        else {
            panic!("expected result object");
        };
        let Some(JsonValue::String(text)) =
            result.iter().find_map(
                |(key, value)| {
                    if key == "text" { Some(value) } else { None }
                },
            )
        else {
            panic!("expected text string");
        };
        assert_eq!(text, "Hello 😃");
    }
}
