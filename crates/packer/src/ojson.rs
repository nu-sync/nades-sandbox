//! JSON that keeps its objects' key order and its numbers' text, for editing Source 2 Viewer's glTF as
//! `export_map.py` edits it (Python's `json` keeps insertion order): glTF-Transform keeps `extras` as they are, so an
//! object's key order reaches the compressed export, and through its hash, the pack.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number as written.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(m) => m.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Python's `setdefault`: the member, inserted at the end as `default` when absent.
    pub fn entry(&mut self, key: &str, default: Json) -> &mut Json {
        let Json::Object(m) = self else { panic!("entry on a non-object") };
        match m.iter().position(|(k, _)| k == key) {
            Some(i) => &mut m[i].1,
            None => {
                m.push((key.to_string(), default));
                &mut m.last_mut().expect("pushed").1
            }
        }
    }

    /// Sets a member: in place when present, else at the end.
    pub fn set(&mut self, key: &str, value: Json) {
        *self.entry(key, Json::Null) = value;
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Json::String(s) = self { Some(s) } else { None }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        if let Json::Array(a) = self { Some(a) } else { None }
    }

    pub fn as_f64(&self) -> Option<f64> {
        if let Json::Number(n) = self { n.parse().ok() } else { None }
    }

    pub fn number(value: impl std::fmt::Display) -> Json {
        Json::Number(value.to_string())
    }

    pub fn object(members: Vec<(&str, Json)>) -> Json {
        Json::Object(members.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => out.push_str(n),
            Json::String(s) => string(s, out),
            Json::Array(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Object(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    string(k, out);
                    out.push_str(": ");
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

/// A string as Python's `json.dumps` writes it (ASCII only).
fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse(text: &str) -> Result<Json> {
    let mut p = Parser { s: text.as_bytes(), at: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.at != p.s.len() {
        bail!("JSON: text after the value at byte {}", p.at);
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.at).is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<()> {
        self.ws();
        if self.s.get(self.at) != Some(&c) {
            bail!("JSON: '{}' expected at byte {}", c as char, self.at);
        }
        self.at += 1;
        Ok(())
    }

    fn value(&mut self) -> Result<Json> {
        self.ws();
        match self.s.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                let mut m = Vec::new();
                self.ws();
                if self.s.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    let v = self.value()?;
                    // Python keeps the last of repeated keys, in the first one's place
                    match m.iter().position(|(key, _): &(String, Json)| *key == k) {
                        Some(i) => m[i].1 = v,
                        None => m.push((k, v)),
                    }
                    self.ws();
                    match self.s.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Object(m));
                        }
                        _ => bail!("JSON: ',' or '}}' expected at byte {}", self.at),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut a = Vec::new();
                self.ws();
                if self.s.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    match self.s.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Array(a));
                        }
                        _ => bail!("JSON: ',' or ']' expected at byte {}", self.at),
                    }
                }
            }
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') if self.s[self.at..].starts_with(b"true") => {
                self.at += 4;
                Ok(Json::Bool(true))
            }
            Some(b'f') if self.s[self.at..].starts_with(b"false") => {
                self.at += 5;
                Ok(Json::Bool(false))
            }
            Some(b'n') if self.s[self.at..].starts_with(b"null") => {
                self.at += 4;
                Ok(Json::Null)
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() || *c == b'N' || *c == b'I' => {
                let start = self.at;
                while self.s.get(self.at).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'+' | b'.')) {
                    self.at += 1;
                }
                Ok(Json::Number(String::from_utf8_lossy(&self.s[start..self.at]).into_owned()))
            }
            _ => bail!("JSON: a value expected at byte {}", self.at),
        }
    }

    fn string(&mut self) -> Result<String> {
        if self.s.get(self.at) != Some(&b'"') {
            bail!("JSON: a string expected at byte {}", self.at);
        }
        self.at += 1;
        let mut out: Vec<u16> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let flush = |bytes: &mut Vec<u8>, out: &mut Vec<u16>| {
            out.extend(String::from_utf8_lossy(bytes).encode_utf16());
            bytes.clear();
        };
        loop {
            let Some(&c) = self.s.get(self.at) else { bail!("JSON: a string is not closed") };
            self.at += 1;
            match c {
                b'"' => {
                    flush(&mut bytes, &mut out);
                    return Ok(String::from_utf16_lossy(&out));
                }
                b'\\' => {
                    flush(&mut bytes, &mut out);
                    let Some(&e) = self.s.get(self.at) else { bail!("JSON: a string is not closed") };
                    self.at += 1;
                    match e {
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.at..self.at + 4).unwrap_or_default())?;
                            out.push(u16::from_str_radix(hex, 16)?);
                            self.at += 4;
                        }
                        b'n' => out.push(10),
                        b't' => out.push(9),
                        b'r' => out.push(13),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        other => out.push(u16::from(other)),
                    }
                }
                _ => bytes.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_numbers_and_strings_survive_a_round_trip() {
        let text = r#"{"b": 1.50, "a": [1e-05, -2, true, null], "s": "caf\u00e9 \"q\"", "o": {"z": {}, "y": []}}"#;
        let v = parse(text).unwrap();
        assert_eq!(v.to_text(), text);
        let mut v = v;
        v.entry("extras", Json::Object(vec![])).set("layers", Json::number(3));
        v.set("b", Json::Bool(false));
        assert_eq!(v.to_text(), r#"{"b": false, "a": [1e-05, -2, true, null], "s": "caf\u00e9 \"q\"", "o": {"z": {}, "y": []}, "extras": {"layers": 3}}"#);
        assert_eq!(parse("{\"k\": 1, \"k\": 2}").unwrap().to_text(), "{\"k\": 2}");
    }
}
