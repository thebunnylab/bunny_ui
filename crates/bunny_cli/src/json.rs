//! JSON, read and written by hand — what `cargo metadata`, `simctl` and
//! `devicectl` answer in, and what `--json` prints.
//!
//! Numbers keep the text they were written with: `devicectl` reports
//! CPU subtypes past what an `i64` holds, and nothing here does
//! arithmetic on them — a field is converted when it is read, by the
//! one who knows what it is.

use std::fmt::Write;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number as written.
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// The members in the order they came — a printout reads the same.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    /// The value down a path of members: `["result", "devices"]`.
    pub fn path(&self, keys: &[&str]) -> Option<&Value> {
        keys.iter().try_fold(self, |value, key| value.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(text) => text.parse().ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Value] {
        match self {
            Value::Array(items) => items,
            _ => &[],
        }
    }

    pub fn members(&self) -> &[(String, Value)] {
        match self {
            Value::Object(members) => members,
            _ => &[],
        }
    }

    /// The string down `keys`, if every step is there.
    pub fn str_at(&self, keys: &[&str]) -> Option<&str> {
        self.path(keys).and_then(Value::as_str)
    }
}

/// Parses one JSON document; trailing text other than whitespace is an
/// error.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader { bytes: text.as_bytes(), at: 0 };
    let value = reader.value(0)?;
    reader.space();
    if reader.at != reader.bytes.len() {
        return Err(reader.error("text after the document"));
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// Deeper than any tool's answer: a bound, so a hostile input cannot
/// overflow the stack.
const MAX_DEPTH: usize = 128;

impl Reader<'_> {
    fn error(&self, what: &str) -> String {
        format!("JSON: {what} at byte {}", self.at)
    }

    fn space(&mut self) {
        while self.at < self.bytes.len() && matches!(self.bytes[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(self.error(&format!("expected `{word}`")))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        self.space();
        match self.peek() {
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.space();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.space();
                    self.expect(":")?;
                    let value = self.value(depth + 1)?;
                    members.push((key, value));
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Value::Object(members));
                        }
                        _ => return Err(self.error("expected `,` or `}`")),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Value::Array(items));
                        }
                        _ => return Err(self.error("expected `,` or `]`")),
                    }
                }
            }
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.expect("true").map(|()| Value::Bool(true)),
            Some(b'f') => self.expect("false").map(|()| Value::Bool(false)),
            Some(b'n') => self.expect("null").map(|()| Value::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.at;
                while self.at < self.bytes.len()
                    && matches!(self.bytes[self.at], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    self.at += 1;
                }
                Ok(Value::Number(String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned()))
            }
            _ => Err(self.error("expected a value")),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.peek() != Some(b'"') {
            return Err(self.error("expected a string"));
        }
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let Some(byte) = self.peek() else { return Err(self.error("unterminated string")) };
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).map_err(|_| self.error("a string that is not UTF-8")),
                b'\\' => {
                    let Some(escape) = self.peek() else { return Err(self.error("unterminated escape")) };
                    self.at += 1;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) && self.bytes[self.at..].starts_with(b"\\u") {
                                self.at += 2;
                                let low = self.hex4()?;
                                code = 0x10000 + ((code - 0xD800) << 10) + (low.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            let c = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut buffer = [0; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                        }
                        _ => return Err(self.error("unknown escape")),
                    }
                }
                byte => out.push(byte),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.bytes.get(self.at..self.at + 4).ok_or_else(|| self.error("short \\u escape"))?;
        let text = std::str::from_utf8(digits).map_err(|_| self.error("bad \\u escape"))?;
        let code = u32::from_str_radix(text, 16).map_err(|_| self.error("bad \\u escape"))?;
        self.at += 4;
        Ok(code)
    }
}

/// A JSON string literal, quoted and escaped.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The value as compact JSON.
pub fn write(value: &Value) -> String {
    match value {
        Value::Null => String::from("null"),
        Value::Bool(value) => value.to_string(),
        Value::Number(text) => text.clone(),
        Value::String(text) => quote(text),
        Value::Array(items) => format!("[{}]", items.iter().map(write).collect::<Vec<_>>().join(",")),
        Value::Object(members) => format!(
            "{{{}}}",
            members.iter().map(|(key, value)| format!("{}:{}", quote(key), write(value))).collect::<Vec<_>>().join(",")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shapes_tools_answer_in() {
        let value = parse(
            r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-27-0":[{"dataPath":"\/Users\/x\/data",
            "udid":"C56759B4","isAvailable":true,"state":"Shutdown","name":"iPhone 18 Pro"}]},"n":null}"#,
        )
        .unwrap();
        let runtime = &value.get("devices").unwrap().members()[0];
        assert_eq!(runtime.0, "com.apple.CoreSimulator.SimRuntime.iOS-27-0");
        let device = &runtime.1.as_array()[0];
        assert_eq!(device.str_at(&["dataPath"]), Some("/Users/x/data"), "`\\/` is a slash");
        assert_eq!(device.get("isAvailable").and_then(Value::as_bool), Some(true));
        assert_eq!(value.get("n"), Some(&Value::Null));
    }

    #[test]
    fn numbers_keep_their_text() {
        let value = parse(r#"{"subtype":18446744071562067980,"type":16777228,"x":-1.5e3}"#).unwrap();
        assert_eq!(value.get("subtype"), Some(&Value::Number(String::from("18446744071562067980"))));
        assert_eq!(value.get("type").and_then(Value::as_u64), Some(16_777_228));
        assert_eq!(value.get("x"), Some(&Value::Number(String::from("-1.5e3"))));
    }

    #[test]
    fn escapes_and_surrogates() {
        let value = parse(r#""tab\t quote\" é 🐰""#).unwrap();
        assert_eq!(value.as_str(), Some("tab\t quote\" é 🐰"));
    }

    #[test]
    fn broken_documents_are_refused() {
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "\"open", "tru", "{} x", &"[".repeat(200)] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn writing_round_trips() {
        let text = r#"{"name":"Ada \"B\"","ok":true,"list":[1,null,"\n"]}"#;
        let value = parse(text).unwrap();
        assert_eq!(parse(&write(&value)).unwrap(), value);
        assert_eq!(write(&value), text);
    }
}
