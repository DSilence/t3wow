//! Lua 5.1 value serialization (bridge -> addon) and a reader for the
//! SavedVariables files WoW writes (addon -> bridge). Values are JSON values:
//! tables with only 1..n integer keys are arrays, others are objects.

use std::fmt::{self, Write as _};

use serde_json::{Map, Number, Value};

const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not", "or",
    "repeat", "return", "then", "true", "until", "while",
];

/// Quote a string as a Lua 5.1 literal. Non-ASCII text is kept as-is (files are UTF-8).
pub fn lua_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Always three digits so a following digit cannot extend the escape.
            '\x00'..='\x1f' | '\x7f' => write!(out, "\\{:03}", ch as u32).unwrap(),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn lua_number(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(0.0);
    if f.is_nan() {
        "(0/0)".into()
    } else if f.is_infinite() {
        if f > 0.0 { "math.huge".into() } else { "-math.huge".into() }
    } else {
        // Shortest representation that reads back as the same double.
        format!("{f}")
    }
}

fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !LUA_KEYWORDS.contains(&key)
}

fn lua_key(key: &str) -> String {
    if is_identifier(key) { key.to_string() } else { format!("[{}]", lua_string(key)) }
}

/// Serialize a value as a Lua expression. `null` object entries are left out.
pub fn to_lua(value: &Value) -> String {
    let mut out = String::new();
    write_lua(&mut out, value, "");
    out
}

fn write_lua(out: &mut String, value: &Value, indent: &str) {
    match value {
        Value::Null => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&lua_number(n)),
        Value::String(s) => out.push_str(&lua_string(s)),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("{}");
                return;
            }
            let inner = format!("{indent} ");
            out.push_str("{\n");
            for item in items {
                out.push_str(&inner);
                write_lua(out, item, &inner);
                out.push_str(",\n");
            }
            out.push_str(indent);
            out.push('}');
        }
        Value::Object(map) => {
            let mut entries = map.iter().filter(|(_, v)| !v.is_null()).peekable();
            if entries.peek().is_none() {
                out.push_str("{}");
                return;
            }
            let inner = format!("{indent} ");
            out.push_str("{\n");
            for (k, v) in entries {
                out.push_str(&inner);
                out.push_str(&lua_key(k));
                out.push_str(" = ");
                write_lua(out, v, &inner);
                out.push_str(",\n");
            }
            out.push_str(indent);
            out.push('}');
        }
    }
}

#[derive(Debug)]
pub struct LuaParseError(pub String);

impl fmt::Display for LuaParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LuaParseError {}

type ParseResult<T> = Result<T, LuaParseError>;

/// Parse the assignments in a SavedVariables file:
///   Name = <table|string|number|boolean|nil>
pub fn parse_saved_variables(source: &[u8]) -> ParseResult<Map<String, Value>> {
    let mut parser = Parser { src: source, pos: 0 };
    let mut result = Map::new();
    parser.skip();
    while !parser.done() {
        let name = parser.identifier()?;
        parser.skip();
        parser.expect(b'=')?;
        let value = parser.value()?;
        result.insert(name, value.unwrap_or(Value::Null));
        parser.skip();
        if parser.peek(0) == Some(b';') {
            parser.pos += 1;
            parser.skip();
        }
    }
    Ok(result)
}

/// A parsed value. Numbers stay `f64` until the table they are in is built,
/// since infinities have no JSON form.
enum Parsed {
    Value(Value),
    Number(f64),
}

impl Parsed {
    fn into_value(self) -> Option<Value> {
        match self {
            Parsed::Value(Value::Null) => None,
            Parsed::Value(v) => Some(v),
            Parsed::Number(n) => number_value(n),
        }
    }
}

fn number_value(n: f64) -> Option<Value> {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        Some(Value::from(n as i64))
    } else {
        Number::from_f64(n).map(Value::Number)
    }
}

/// Table keys: integers stay integers so `{ "a", "b" }` becomes an array.
#[derive(PartialEq)]
enum Key {
    Index(i64),
    Name(String),
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn done(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn fail<T>(&self, message: impl fmt::Display) -> ParseResult<T> {
        Err(LuaParseError(format!("{message} at byte {}", self.pos)))
    }

    fn expect(&mut self, ch: u8) -> ParseResult<()> {
        if self.peek(0) != Some(ch) {
            let found = self.peek(0).map(|b| (b as char).to_string()).unwrap_or_default();
            return self.fail(format!("expected '{}' but found '{found}'", ch as char));
        }
        self.pos += 1;
        Ok(())
    }

    fn skip(&mut self) {
        loop {
            match self.peek(0) {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.pos += 1,
                Some(b'-') if self.peek(1) == Some(b'-') => {
                    self.pos += 2;
                    if let Some(level) = self.long_bracket_level() {
                        // An unterminated comment runs to the end of the file.
                        if self.long_bracket(level).is_err() {
                            self.pos = self.src.len();
                        }
                    } else {
                        while !self.done() && self.peek(0) != Some(b'\n') {
                            self.pos += 1;
                        }
                    }
                }
                _ => return,
            }
        }
    }

    fn identifier(&mut self) -> ParseResult<String> {
        let start = self.pos;
        while matches!(self.peek(0), Some(c) if c.is_ascii_alphanumeric() || c == b'_') {
            self.pos += 1;
        }
        if start == self.pos {
            return self.fail("expected identifier");
        }
        Ok(String::from_utf8_lossy(&self.src[start..self.pos]).into_owned())
    }

    fn value(&mut self) -> ParseResult<Option<Value>> {
        Ok(self.parsed()?.into_value())
    }

    fn parsed(&mut self) -> ParseResult<Parsed> {
        self.skip();
        match self.peek(0) {
            Some(b'{') => return self.table().map(Parsed::Value),
            Some(b'"' | b'\'') => return self.quoted().map(|s| Parsed::Value(Value::String(s))),
            Some(b'[') => {
                if let Some(level) = self.long_bracket_level() {
                    return self.long_bracket(level).map(|s| Parsed::Value(Value::String(s)));
                }
            }
            Some(c) if c == b'-' || c == b'.' || c.is_ascii_digit() => return self.number().map(Parsed::Number),
            _ => {}
        }
        let word = self.identifier()?;
        match word.as_str() {
            "true" => Ok(Parsed::Value(Value::Bool(true))),
            "false" => Ok(Parsed::Value(Value::Bool(false))),
            "nil" => Ok(Parsed::Value(Value::Null)),
            "math" => {
                self.expect(b'.')?;
                if self.identifier()? == "huge" {
                    Ok(Parsed::Number(f64::INFINITY))
                } else {
                    self.fail("unexpected word 'math'")
                }
            }
            _ => self.fail(format!("unexpected word '{word}'")),
        }
    }

    fn number(&mut self) -> ParseResult<f64> {
        let negative = self.peek(0) == Some(b'-');
        if negative {
            self.pos += 1;
            while matches!(self.peek(0), Some(b' ' | b'\t')) {
                self.pos += 1;
            }
        }
        let n = if self.src[self.pos..].starts_with(b"math.huge") {
            self.pos += "math.huge".len();
            f64::INFINITY
        } else if self.peek(0) == Some(b'0') && matches!(self.peek(1), Some(b'x' | b'X')) {
            self.pos += 2;
            let start = self.pos;
            while matches!(self.peek(0), Some(c) if c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let digits = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
            match u64::from_str_radix(digits, 16) {
                Ok(n) => n as f64,
                Err(_) => return self.fail("bad number"),
            }
        } else {
            let start = self.pos;
            while matches!(self.peek(0), Some(c) if c.is_ascii_digit() || c == b'.') {
                self.pos += 1;
            }
            if matches!(self.peek(0), Some(b'e' | b'E')) {
                self.pos += 1;
                if matches!(self.peek(0), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                while matches!(self.peek(0), Some(c) if c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            match std::str::from_utf8(&self.src[start..self.pos]).ok().and_then(|s| s.parse::<f64>().ok()) {
                Some(n) => n,
                None => return self.fail("bad number"),
            }
        };
        Ok(if negative { -n } else { n })
    }

    /// Level of a long bracket opening at pos (`[[` = 0, `[=[` = 1).
    fn long_bracket_level(&self) -> Option<usize> {
        if self.peek(0) != Some(b'[') {
            return None;
        }
        let mut level = 0;
        while self.peek(1 + level) == Some(b'=') {
            level += 1;
        }
        (self.peek(1 + level) == Some(b'[')).then_some(level)
    }

    fn long_bracket(&mut self, level: usize) -> ParseResult<String> {
        self.pos += level + 2;
        if self.peek(0) == Some(b'\r') {
            self.pos += 1;
        }
        if self.peek(0) == Some(b'\n') {
            self.pos += 1;
        }
        let close: Vec<u8> = std::iter::once(b']').chain(std::iter::repeat_n(b'=', level)).chain(*b"]").collect();
        let Some(offset) = self.src[self.pos..].windows(close.len()).position(|w| w == close) else {
            return self.fail("unterminated long bracket");
        };
        let text = String::from_utf8_lossy(&self.src[self.pos..self.pos + offset]).into_owned();
        self.pos += offset + close.len();
        Ok(text)
    }

    fn quoted(&mut self) -> ParseResult<String> {
        let quote = self.src[self.pos];
        self.pos += 1;
        let mut bytes = Vec::new();
        loop {
            let Some(b) = self.peek(0) else { return self.fail("unterminated string") };
            self.pos += 1;
            if b == quote {
                break;
            }
            if b != b'\\' {
                bytes.push(b);
                continue;
            }
            let Some(e) = self.peek(0) else { return self.fail("unterminated escape") };
            self.pos += 1;
            match e {
                b'n' | b'\n' => bytes.push(b'\n'),
                b't' => bytes.push(b'\t'),
                b'r' => bytes.push(b'\r'),
                b'a' => bytes.push(7),
                b'b' => bytes.push(8),
                b'f' => bytes.push(12),
                b'v' => bytes.push(11),
                b'0'..=b'9' => {
                    let mut n = u32::from(e - b'0');
                    for _ in 0..2 {
                        match self.peek(0) {
                            Some(d @ b'0'..=b'9') => {
                                n = n * 10 + u32::from(d - b'0');
                                self.pos += 1;
                            }
                            _ => break,
                        }
                    }
                    bytes.push((n & 0xff) as u8);
                }
                other => bytes.push(other),
            }
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn table(&mut self) -> ParseResult<Value> {
        self.expect(b'{')?;
        let mut entries: Vec<(Key, Value)> = Vec::new();
        let mut next_index = 1;
        loop {
            self.skip();
            if self.peek(0) == Some(b'}') {
                self.pos += 1;
                break;
            }
            let key = if self.peek(0) == Some(b'[') && self.long_bracket_level().is_none() {
                self.pos += 1;
                let key = match self.parsed()? {
                    Parsed::Value(Value::String(s)) => Key::Name(s),
                    Parsed::Number(n) if n.fract() == 0.0 && n.is_finite() => Key::Index(n as i64),
                    Parsed::Number(n) => Key::Name(n.to_string()),
                    _ => return self.fail("unsupported key type"),
                };
                self.skip();
                self.expect(b']')?;
                self.skip();
                self.expect(b'=')?;
                key
            } else if matches!(self.peek(0), Some(c) if c.is_ascii_alphabetic() || c == b'_') && self.is_named_field() {
                let name = self.identifier()?;
                self.skip();
                self.expect(b'=')?;
                Key::Name(name)
            } else {
                next_index += 1;
                Key::Index(next_index - 1)
            };
            let value = self.value()?;
            entries.retain(|(k, _)| *k != key);
            if let Some(value) = value {
                entries.push((key, value));
            }
            self.skip();
            if matches!(self.peek(0), Some(b',' | b';')) {
                self.pos += 1;
            }
        }
        Ok(build_table(entries))
    }

    /// True when the identifier at pos is followed by `=` (a `name = value` field).
    fn is_named_field(&mut self) -> bool {
        let save = self.pos;
        let named = self.identifier().is_ok() && {
            self.skip();
            self.peek(0) == Some(b'=') && self.peek(1) != Some(b'=')
        };
        self.pos = save;
        named
    }
}

fn build_table(entries: Vec<(Key, Value)>) -> Value {
    let size = entries.len();
    let mut slots: Vec<Option<Value>> = vec![None; size];
    let mut is_array = size > 0;
    for (key, _) in &entries {
        match key {
            Key::Index(i) if *i >= 1 && (*i as usize) <= size => {}
            _ => {
                is_array = false;
                break;
            }
        }
    }
    if is_array {
        for (key, value) in entries {
            if let Key::Index(i) = key {
                slots[i as usize - 1] = Some(value);
            }
        }
        return Value::Array(slots.into_iter().map(|v| v.unwrap_or(Value::Null)).collect());
    }
    let mut map = Map::new();
    for (key, value) in entries {
        let key = match key {
            Key::Index(i) => i.to_string(),
            Key::Name(name) => name,
        };
        map.insert(key, value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quotes_strings_safely() {
        assert_eq!(lua_string("a\"b\\c\nd\u{0}1"), r#""a\"b\\c\nd\0001""#);
        assert_eq!(lua_string("héllo ✓"), "\"héllo ✓\"");
    }

    #[test]
    fn serializes_tables() {
        let value = json!({ "and": 1, "ok": true, "list": ["x", 2.5], "skip": null, "empty": {} });
        assert_eq!(
            to_lua(&value),
            "{\n [\"and\"] = 1,\n empty = {},\n list = {\n  \"x\",\n  2.5,\n },\n ok = true,\n}"
        );
    }

    #[test]
    fn parses_saved_variables() {
        let source = br#"
-- a comment
T3WowDB = {
["instanceId"] = "inst-1",
["outbox"] = {
{
["seq"] = 7,
["text"] = "line\nnext \"quoted\" \226\156\147",
}, -- [1]
},
["watch"] = { ["t1"] = 1760000000, },
["neg"] = -3.5,
["hex"] = 0x1F,
["big"] = math.huge,
["long"] = [==[raw ]] text]==],
["flags"] = { true, false, nil, },
}
Other = nil
"#;
        let vars = parse_saved_variables(source).unwrap();
        let db = &vars["T3WowDB"];
        assert_eq!(db["instanceId"], "inst-1");
        assert_eq!(db["outbox"][0]["seq"], 7);
        assert_eq!(db["outbox"][0]["text"], "line\nnext \"quoted\" ✓");
        assert_eq!(db["watch"]["t1"], 1760000000);
        assert_eq!(db["neg"], -3.5);
        assert_eq!(db["hex"], 31);
        assert!(db.get("big").is_none(), "infinity has no JSON form");
        assert_eq!(db["long"], "raw ]] text");
        assert_eq!(db["flags"], json!([true, false]));
        assert_eq!(vars["Other"], Value::Null);
    }

    #[test]
    fn round_trips() {
        let value = json!({ "a": ["x\u{1}y", { "b": -2 }], "c": "[[x]]" });
        let source = format!("V = {}", to_lua(&value));
        assert_eq!(parse_saved_variables(source.as_bytes()).unwrap()["V"], value);
    }

    #[test]
    fn reports_errors() {
        assert!(parse_saved_variables(b"T3WowDB = {").is_err());
        assert!(parse_saved_variables(b"T3WowDB = \"open").is_err());
    }
}
