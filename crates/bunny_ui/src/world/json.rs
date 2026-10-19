//! A small JSON reader for the glTF door: values, nothing more.
//!
//! It refuses rather than guesses. Nesting deeper than [`MAX_DEPTH`] is
//! an error, never a stack overflow; every malformed input is an error,
//! never a panic.

/// Nesting a document may reach.
pub const MAX_DEPTH: usize = 64;

/// A JSON value. An object keeps its members in document order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// Why a text is not JSON: what went wrong, and the byte it went wrong at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub what: &'static str,
    pub at: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let mut reader = Reader { bytes: text.as_bytes(), at: 0 };
        let value = reader.value(0)?;
        reader.space();
        if reader.at != reader.bytes.len() {
            return Err(reader.error("text after the value"));
        }
        Ok(value)
    }

    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// A whole number that fits an index.
    pub fn as_index(&self) -> Option<usize> {
        let value = self.as_f64()?;
        (value >= 0.0 && value.fract() == 0.0 && value <= u32::MAX as f64).then_some(value as usize)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }

    /// The numbers of an array, or `None` when any item is not one.
    pub fn as_numbers(&self) -> Option<Vec<f64>> {
        self.as_array().iter().map(Json::as_f64).collect()
    }

    /// The value written back as compact JSON — how an asset's `extras`
    /// reach the app.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Number(value) => {
                if value.fract() == 0.0 && value.abs() < 1e15 {
                    out.push_str(&format!("{}", *value as i64));
                } else {
                    out.push_str(&format!("{value}"));
                }
            }
            Json::String(text) => write_string(out, text),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(members) => {
                out.push('{');
                for (i, (name, value)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(out, name);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn error(&self, what: &'static str) -> JsonError {
        JsonError { what, at: self.at }
    }

    fn space(&mut self) {
        while self.at < self.bytes.len() && matches!(self.bytes[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self, word: &[u8], value: Json) -> Result<Json, JsonError> {
        if self.bytes[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("unknown word"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        self.space();
        match self.bytes.get(self.at) {
            None => Err(self.error("the text ended")),
            Some(b'n') => self.word(b"null", Json::Null),
            Some(b't') => self.word(b"true", Json::Bool(true)),
            Some(b'f') => self.word(b"false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.eat(b']') {
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.space();
                    if self.eat(b']') {
                        return Ok(Json::Array(items));
                    }
                    if !self.eat(b',') {
                        return Err(self.error("expected , or ]"));
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.space();
                if self.eat(b'}') {
                    return Ok(Json::Object(members));
                }
                loop {
                    self.space();
                    if self.bytes.get(self.at) != Some(&b'"') {
                        return Err(self.error("expected a name"));
                    }
                    let name = self.string()?;
                    self.space();
                    if !self.eat(b':') {
                        return Err(self.error("expected :"));
                    }
                    let value = self.value(depth + 1)?;
                    members.push((name, value));
                    self.space();
                    if self.eat(b'}') {
                        return Ok(Json::Object(members));
                    }
                    if !self.eat(b',') {
                        return Err(self.error("expected , or }"));
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected byte")),
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        while self.at < self.bytes.len() && matches!(self.bytes[self.at], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| self.error("bad number"))?;
        let value: f64 = text.parse().map_err(|_| JsonError { what: "bad number", at: start })?;
        if !value.is_finite() {
            return Err(JsonError { what: "number out of range", at: start });
        }
        Ok(Json::Number(value))
    }

    fn string(&mut self) -> Result<String, JsonError> {
        // the opening quote
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else { return Err(self.error("the string never closed")) };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.at += 1;
                    let Some(&escape) = self.bytes.get(self.at) else { return Err(self.error("the string never closed")) };
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let high = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&high) {
                                // a surrogate pair: the low half must follow
                                if !(self.eat(b'\\') && self.eat(b'u')) {
                                    return Err(self.error("a lone surrogate"));
                                }
                                let low = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return Err(self.error("a lone surrogate"));
                                }
                                0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                high
                            };
                            out.push(char::from_u32(code).ok_or_else(|| self.error("not a character"))?);
                        }
                        _ => return Err(self.error("unknown escape")),
                    }
                }
                0..=0x1F => return Err(self.error("a control byte in a string")),
                _ => {
                    // copy one UTF-8 character whole
                    let rest = std::str::from_utf8(&self.bytes[self.at..]).or_else(|e| std::str::from_utf8(&self.bytes[self.at..self.at + e.valid_up_to()]));
                    let rest = rest.map_err(|_| self.error("bad UTF-8"))?;
                    let Some(c) = rest.chars().next() else { return Err(self.error("bad UTF-8")) };
                    out.push(c);
                    self.at += c.len_utf8();
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let digits = self.bytes.get(self.at..self.at + 4).ok_or_else(|| self.error("a short escape"))?;
        let text = std::str::from_utf8(digits).map_err(|_| self.error("a bad escape"))?;
        let value = u32::from_str_radix(text, 16).map_err(|_| self.error("a bad escape"))?;
        self.at += 4;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse_and_write_back() {
        let text = r#"{"a":[1,2.5,-3e2],"b":{"c":"x\"y\u00e9\ud83d\ude00"},"d":true,"e":null}"#;
        let json = Json::parse(text).expect("valid");
        assert_eq!(json.get("a").and_then(Json::as_numbers), Some(vec![1.0, 2.5, -300.0]));
        assert_eq!(json.get("b").and_then(|b| b.get("c")).and_then(Json::as_str), Some("x\"yé😀"));
        assert_eq!(Json::parse(&json.to_text()).expect("round trip"), json);
    }

    #[test]
    fn json_refuses_deep_nesting() {
        let deep = "[".repeat(MAX_DEPTH + 2) + &"]".repeat(MAX_DEPTH + 2);
        assert_eq!(Json::parse(&deep).unwrap_err().what, "nesting too deep");
        let fine = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(Json::parse(&fine).is_ok());
    }

    #[test]
    fn malformed_text_is_an_error() {
        for text in ["", "{", "[1,]", "{\"a\" 1}", "\"open", "nul", "1e999", "[1] 2", "\"\\x\"", "{\"a\":\"\\ud800\"}"] {
            assert!(Json::parse(text).is_err(), "{text:?}");
        }
    }
}
