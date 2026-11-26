//! Strict JSON for the signed update profile and its canonical serialization.
//!
//! The profile allows objects, arrays, booleans, non-negative integers up to
//! 2^53 - 1 and strings of printable ASCII (0x20..=0x7E) only. Duplicate object
//! keys, floats, exponents, null and nesting deeper than [`MAX_DEPTH`] are
//! rejected. `serde_json` is not used because it accepts duplicate keys (last
//! one wins) and passes DEL through where the Python fixture escapes it.
//!
//! [`canonical`] reproduces Python's
//! `json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=True)` for
//! such values: keys sorted bytewise, no whitespace, and only `"` and `\`
//! escaped.
use std::collections::BTreeMap;

use super::{MAX_INTEGER, Reject};

const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Bool(bool),
    Int(u64),
    /// Always printable ASCII when produced by [`parse`].
    Str(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

pub(crate) fn parse(raw: &[u8]) -> Result<Value, Reject> {
    let mut parser = Parser { raw, pos: 0 };
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.pos == raw.len() {
        Ok(value)
    } else {
        Err(Reject("invalid-json"))
    }
}

pub(crate) fn canonical(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Bool(flag) => out.extend_from_slice(if *flag { b"true" } else { b"false" }),
        Value::Int(number) => out.extend_from_slice(number.to_string().as_bytes()),
        Value::Str(text) => write_str(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write(item, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            out.push(b'{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_str(key, out);
                out.push(b':');
                write(item, out);
            }
            out.push(b'}');
        }
    }
}

fn write_str(text: &str, out: &mut Vec<u8>) {
    debug_assert!(
        text.bytes().all(printable),
        "canonical strings are printable ASCII"
    );
    out.push(b'"');
    for byte in text.bytes() {
        if matches!(byte, b'"' | b'\\') {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b'"');
}

fn printable(byte: u8) -> bool {
    (0x20..=0x7e).contains(&byte)
}

struct Parser<'a> {
    raw: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.raw.get(self.pos).copied()
    }

    fn next(&mut self) -> Result<u8, Reject> {
        let byte = self.peek().ok_or(Reject("invalid-json"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// Consumes `byte` after optional whitespace, if it is next.
    fn eat(&mut self, byte: u8) -> bool {
        self.whitespace();
        let found = self.peek() == Some(byte);
        if found {
            self.pos += 1;
        }
        found
    }

    fn expect(&mut self, byte: u8) -> Result<(), Reject> {
        if self.eat(byte) {
            Ok(())
        } else {
            Err(Reject("invalid-json"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Reject> {
        self.whitespace();
        match self.peek().ok_or(Reject("invalid-json"))? {
            b'{' | b'[' if depth >= MAX_DEPTH => Err(Reject("json-depth")),
            b'{' => self.object(depth + 1),
            b'[' => self.array(depth + 1),
            b'"' => self.string().map(Value::Str),
            b't' => self.literal(b"true", Value::Bool(true)),
            b'f' => self.literal(b"false", Value::Bool(false)),
            b'0'..=b'9' | b'-' | b'.' => self.number(),
            _ => Err(Reject("invalid-json")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Reject> {
        self.pos += 1;
        let mut map = BTreeMap::new();
        if self.eat(b'}') {
            return Ok(Value::Object(map));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(Reject("invalid-json"));
            }
            let key = self.string()?;
            self.expect(b':')?;
            let item = self.value(depth)?;
            if map.insert(key, item).is_some() {
                return Err(Reject("duplicate-json-key"));
            }
            if self.eat(b'}') {
                return Ok(Value::Object(map));
            }
            self.expect(b',')?;
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Reject> {
        self.pos += 1;
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value(depth)?);
            if self.eat(b']') {
                return Ok(Value::Array(items));
            }
            self.expect(b',')?;
        }
    }

    fn literal(&mut self, word: &[u8], value: Value) -> Result<Value, Reject> {
        if self.raw[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(Reject("invalid-json"))
        }
    }

    /// Accepts only `0` or a digit run without a leading zero, within 2^53 - 1.
    fn number(&mut self) -> Result<Value, Reject> {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.pos += 1;
        }
        let digits = &self.raw[start..self.pos];
        let valid = digits.iter().all(u8::is_ascii_digit) && (digits == b"0" || digits[0] != b'0');
        std::str::from_utf8(digits)
            .ok()
            .filter(|_| valid)
            .and_then(|text| text.parse().ok())
            .filter(|number| *number <= MAX_INTEGER)
            .map(Value::Int)
            .ok_or(Reject("json-number"))
    }

    fn string(&mut self) -> Result<String, Reject> {
        self.pos += 1;
        let mut text = String::new();
        loop {
            let byte = match self.next()? {
                b'"' => return Ok(text),
                b'\\' => match self.next()? {
                    byte @ (b'"' | b'\\' | b'/') => byte,
                    b'u' => self.unicode_escape()?,
                    b'b' | b'f' | b'n' | b'r' | b't' => return Err(Reject("json-string")),
                    _ => return Err(Reject("invalid-json")),
                },
                byte => byte,
            };
            if !printable(byte) {
                return Err(Reject("json-string"));
            }
            text.push(char::from(byte));
        }
    }

    /// Decodes `\uXXXX`; anything outside ASCII fails the printable check later.
    fn unicode_escape(&mut self) -> Result<u8, Reject> {
        let digits = self
            .raw
            .get(self.pos..self.pos + 4)
            .ok_or(Reject("invalid-json"))?;
        self.pos += 4;
        let code = std::str::from_utf8(digits)
            .ok()
            .filter(|text| text.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|text| u16::from_str_radix(text, 16).ok())
            .ok_or(Reject("invalid-json"))?;
        u8::try_from(code).map_err(|_| Reject("json-string"))
    }
}
