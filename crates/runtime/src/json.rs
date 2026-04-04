use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    message: String,
}

impl JsonError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for JsonError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for JsonError {}

impl JsonValue {
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Null => "null".to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => render_string(value),
            Self::Array(values) => {
                let rendered = values
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("[{rendered}]")
            }
            Self::Object(entries) => {
                let rendered = entries
                    .iter()
                    .map(|(key, value)| format!("{}:{}", render_string(key), value.render()))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{rendered}}}")
            }
        }
    }

    pub fn parse(source: &str) -> Result<Self, JsonError> {
        let mut parser = Parser::new(source);
        let value = parser.parse_value()?;
        parser.skip_whitespace();
        if parser.is_eof() {
            Ok(value)
        } else {
            Err(JsonError::new("unexpected trailing content"))
        }
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }
}

fn render_string(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push('"');
    for ch in value.chars() {
        match ch {
            '"' => rendered.push_str("\\\""),
            '\\' => rendered.push_str("\\\\"),
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            '\u{08}' => rendered.push_str("\\b"),
            '\u{0C}' => rendered.push_str("\\f"),
            control if control.is_control() => push_unicode_escape(&mut rendered, control),
            plain => rendered.push(plain),
        }
    }
    rendered.push('"');
    rendered
}

fn push_unicode_escape(rendered: &mut String, control: char) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    rendered.push_str("\\u");
    let value = u32::from(control);
    for shift in [12_u32, 8, 4, 0] {
        let nibble = ((value >> shift) & 0xF) as usize;
        rendered.push(char::from(HEX[nibble]));
    }
}

struct Parser<'a> {
    chars: Vec<char>,
    index: usize,
    _source: &'a str,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            index: 0,
            _source: source,
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, JsonError> {
        self.skip_whitespace();
        match self.peek() {
            Some('n') => self.parse_literal("null", JsonValue::Null),
            Some('t') => self.parse_literal("true", JsonValue::Bool(true)),
            Some('f') => self.parse_literal("false", JsonValue::Bool(false)),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('[') => self.parse_array(),
            Some('{') => self.parse_object(),
            Some('-' | '0'..='9') => self.parse_number().map(JsonValue::Number),
            Some(other) => Err(JsonError::new(format!("unexpected character: {other}"))),
            None => Err(JsonError::new("unexpected end of input")),
        }
    }

    fn parse_literal(&mut self, expected: &str, value: JsonValue) -> Result<JsonValue, JsonError> {
        for expected_char in expected.chars() {
            if self.next() != Some(expected_char) {
                return Err(JsonError::new(format!(
                    "invalid literal: expected {expected}"
                )));
            }
        }
        Ok(value)
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.expect('"')?;
        let mut value = String::new();
        while let Some(ch) = self.next() {
            match ch {
                '"' => return Ok(value),
                '\\' => value.push(self.parse_escape()?),
                plain => value.push(plain),
            }
        }
        Err(JsonError::new("unterminated string"))
    }

    fn parse_escape(&mut self) -> Result<char, JsonError> {
        match self.next() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('/') => Ok('/'),
            Some('b') => Ok('\u{08}'),
            Some('f') => Ok('\u{0C}'),
            Some('n') => Ok('\n'),
            Some('r') => Ok('\r'),
            Some('t') => Ok('\t'),
            Some('u') => self.parse_unicode_escape(),
            Some(other) => Err(JsonError::new(format!("invalid escape sequence: {other}"))),
            None => Err(JsonError::new("unexpected end of input in escape sequence")),
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, JsonError> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let Some(ch) = self.next() else {
                return Err(JsonError::new("unexpected end of input in unicode escape"));
            };
            value = (value << 4)
                | ch.to_digit(16)
                    .ok_or_else(|| JsonError::new("invalid unicode escape"))?;
        }
        char::from_u32(value).ok_or_else(|| JsonError::new("invalid unicode scalar value"))
    }

    fn parse_array(&mut self) -> Result<JsonValue, JsonError> {
        self.expect('[')?;
        let mut values = Vec::new();
        loop {
            self.skip_whitespace();
            if self.try_consume(']') {
                break;
            }
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.try_consume(']') {
                break;
            }
            self.expect(',')?;
        }
        Ok(JsonValue::Array(values))
    }

    fn parse_object(&mut self) -> Result<JsonValue, JsonError> {
        self.expect('{')?;
        let mut entries = BTreeMap::new();
        loop {
            self.skip_whitespace();
            if self.try_consume('}') {
                break;
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(':')?;
            let value = self.parse_value()?;
            entries.insert(key, value);
            self.skip_whitespace();
            if self.try_consume('}') {
                break;
            }
            self.expect(',')?;
        }
        Ok(JsonValue::Object(entries))
    }

    fn parse_number(&mut self) -> Result<i64, JsonError> {
        let mut value = String::new();
        if self.try_consume('-') {
            value.push('-');
        }

        while let Some(ch @ '0'..='9') = self.peek() {
            value.push(ch);
            self.index += 1;
        }

        if value.is_empty() || value == "-" {
            return Err(JsonError::new("invalid number"));
        }

        value
            .parse::<i64>()
            .map_err(|_| JsonError::new("number out of range"))
    }

    fn expect(&mut self, expected: char) -> Result<(), JsonError> {
        match self.next() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => Err(JsonError::new(format!(
                "expected '{expected}', found '{actual}'"
            ))),
            None => Err(JsonError::new(format!(
                "expected '{expected}', found end of input"
            ))),
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

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\n' | '\r' | '\t')) {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        Some(ch)
    }

    fn is_eof(&self) -> bool {
        self.index >= self.chars.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{render_string, JsonError, JsonValue};
    use std::collections::BTreeMap;

    #[test]
    fn renders_and_parses_json_values() {
        let mut object = BTreeMap::new();
        object.insert("flag".to_string(), JsonValue::Bool(true));
        object.insert(
            "items".to_string(),
            JsonValue::Array(vec![
                JsonValue::Number(4),
                JsonValue::String("ok".to_string()),
            ]),
        );

        let rendered = JsonValue::Object(object).render();
        let parsed = JsonValue::parse(&rendered).expect("json should parse");

        assert_eq!(parsed.as_object().expect("object").len(), 2);
    }

    #[test]
    fn escapes_control_characters() {
        assert_eq!(render_string("a\n\t\"b"), "\"a\\n\\t\\\"b\"");
    }

    // --- render() tests ---

    #[test]
    fn render_null() {
        assert_eq!(JsonValue::Null.render(), "null");
    }

    #[test]
    fn render_bool_true() {
        assert_eq!(JsonValue::Bool(true).render(), "true");
    }

    #[test]
    fn render_bool_false() {
        assert_eq!(JsonValue::Bool(false).render(), "false");
    }

    #[test]
    fn render_number_positive() {
        assert_eq!(JsonValue::Number(42).render(), "42");
    }

    #[test]
    fn render_number_negative() {
        assert_eq!(JsonValue::Number(-7).render(), "-7");
    }

    #[test]
    fn render_number_zero() {
        assert_eq!(JsonValue::Number(0).render(), "0");
    }

    #[test]
    fn render_empty_string() {
        assert_eq!(JsonValue::String(String::new()).render(), "\"\"");
    }

    #[test]
    fn render_string_with_quotes() {
        assert_eq!(
            JsonValue::String("say \"hi\"".to_string()).render(),
            "\"say \\\"hi\\\"\""
        );
    }

    #[test]
    fn render_empty_array() {
        assert_eq!(JsonValue::Array(vec![]).render(), "[]");
    }

    #[test]
    fn render_array_with_mixed_types() {
        let arr = JsonValue::Array(vec![
            JsonValue::Number(1),
            JsonValue::Bool(false),
            JsonValue::Null,
        ]);
        assert_eq!(arr.render(), "[1,false,null]");
    }

    #[test]
    fn render_empty_object() {
        assert_eq!(JsonValue::Object(BTreeMap::new()).render(), "{}");
    }

    #[test]
    fn render_object_with_entries() {
        let mut map = BTreeMap::new();
        map.insert("a".to_string(), JsonValue::Number(1));
        let rendered = JsonValue::Object(map).render();
        assert_eq!(rendered, "{\"a\":1}");
    }

    // --- parse() tests ---

    #[test]
    fn parse_null() {
        assert_eq!(JsonValue::parse("null").unwrap(), JsonValue::Null);
    }

    #[test]
    fn parse_true() {
        assert_eq!(JsonValue::parse("true").unwrap(), JsonValue::Bool(true));
    }

    #[test]
    fn parse_false() {
        assert_eq!(JsonValue::parse("false").unwrap(), JsonValue::Bool(false));
    }

    #[test]
    fn parse_positive_number() {
        assert_eq!(JsonValue::parse("123").unwrap(), JsonValue::Number(123));
    }

    #[test]
    fn parse_negative_number() {
        assert_eq!(JsonValue::parse("-5").unwrap(), JsonValue::Number(-5));
    }

    #[test]
    fn parse_zero() {
        assert_eq!(JsonValue::parse("0").unwrap(), JsonValue::Number(0));
    }

    #[test]
    fn parse_simple_string() {
        assert_eq!(
            JsonValue::parse("\"hello\"").unwrap(),
            JsonValue::String("hello".to_string())
        );
    }

    #[test]
    fn parse_string_with_escape_sequences() {
        let v = JsonValue::parse("\"a\\nb\\tc\"").unwrap();
        assert_eq!(v, JsonValue::String("a\nb\tc".to_string()));
    }

    #[test]
    fn parse_string_with_backslash_and_quote() {
        let v = JsonValue::parse("\"back\\\\slash \\\"quote\\\"\"").unwrap();
        assert_eq!(v, JsonValue::String("back\\slash \"quote\"".to_string()));
    }

    #[test]
    fn parse_string_with_forward_slash_escape() {
        let v = JsonValue::parse("\"a\\/b\"").unwrap();
        assert_eq!(v, JsonValue::String("a/b".to_string()));
    }

    #[test]
    fn parse_string_with_unicode_escape() {
        let v = JsonValue::parse("\"\\u0041\"").unwrap();
        assert_eq!(v, JsonValue::String("A".to_string()));
    }

    #[test]
    fn parse_empty_array() {
        assert_eq!(JsonValue::parse("[]").unwrap(), JsonValue::Array(vec![]));
    }

    #[test]
    fn parse_array_of_numbers() {
        let v = JsonValue::parse("[1,2,3]").unwrap();
        assert_eq!(
            v,
            JsonValue::Array(vec![
                JsonValue::Number(1),
                JsonValue::Number(2),
                JsonValue::Number(3)
            ])
        );
    }

    #[test]
    fn parse_empty_object() {
        assert_eq!(
            JsonValue::parse("{}").unwrap(),
            JsonValue::Object(BTreeMap::new())
        );
    }

    #[test]
    fn parse_object_with_string_value() {
        let v = JsonValue::parse("{\"key\":\"value\"}").unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(
            obj.get("key"),
            Some(&JsonValue::String("value".to_string()))
        );
    }

    #[test]
    fn parse_nested_object() {
        let v = JsonValue::parse("{\"outer\":{\"inner\":42}}").unwrap();
        let outer = v.as_object().unwrap();
        let inner = outer.get("outer").unwrap().as_object().unwrap();
        assert_eq!(inner.get("inner"), Some(&JsonValue::Number(42)));
    }

    #[test]
    fn parse_whitespace_handling() {
        let v = JsonValue::parse("  {  \"k\"  :  1  }  ").unwrap();
        assert!(v.as_object().is_some());
    }

    // --- error paths ---

    #[test]
    fn parse_error_trailing_content() {
        let result = JsonValue::parse("null extra");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("trailing"));
    }

    #[test]
    fn parse_error_unexpected_char() {
        let result = JsonValue::parse("@invalid");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_empty_input() {
        let result = JsonValue::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_unterminated_string() {
        let result = JsonValue::parse("\"unterminated");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_invalid_escape() {
        let result = JsonValue::parse("\"bad\\xescape\"");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_invalid_unicode_escape() {
        let result = JsonValue::parse("\"\\uzzzz\"");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_number_only_minus() {
        let result = JsonValue::parse("-");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_incomplete_literal() {
        let result = JsonValue::parse("nul");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_object_missing_colon() {
        let result = JsonValue::parse("{\"key\" \"value\"}");
        assert!(result.is_err());
    }

    // --- accessor tests ---

    #[test]
    fn as_str_on_string() {
        let v = JsonValue::String("hello".to_string());
        assert_eq!(v.as_str(), Some("hello"));
    }

    #[test]
    fn as_str_on_non_string() {
        let v = JsonValue::Number(1);
        assert_eq!(v.as_str(), None);
    }

    #[test]
    fn as_bool_on_bool() {
        assert_eq!(JsonValue::Bool(true).as_bool(), Some(true));
        assert_eq!(JsonValue::Bool(false).as_bool(), Some(false));
    }

    #[test]
    fn as_bool_on_non_bool() {
        assert_eq!(JsonValue::Null.as_bool(), None);
    }

    #[test]
    fn as_i64_on_number() {
        assert_eq!(JsonValue::Number(-99).as_i64(), Some(-99));
    }

    #[test]
    fn as_i64_on_non_number() {
        assert_eq!(JsonValue::String("x".to_string()).as_i64(), None);
    }

    #[test]
    fn as_array_on_array() {
        let v = JsonValue::Array(vec![JsonValue::Null]);
        assert!(v.as_array().is_some());
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[test]
    fn as_array_on_non_array() {
        assert!(JsonValue::Null.as_array().is_none());
    }

    #[test]
    fn as_object_on_non_object() {
        assert!(JsonValue::Bool(true).as_object().is_none());
    }

    // --- JsonError ---

    #[test]
    fn json_error_display() {
        let err = JsonError::new("something went wrong");
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn json_error_implements_std_error() {
        let err = JsonError::new("error");
        let _: &dyn std::error::Error = &err;
    }

    // --- render_string edge cases ---

    #[test]
    fn render_string_escapes_backspace() {
        let s = "\u{08}"; // backspace
        let rendered = render_string(s);
        assert!(rendered.contains("\\b"));
    }

    #[test]
    fn render_string_escapes_form_feed() {
        let s = "\u{0C}"; // form feed
        let rendered = render_string(s);
        assert!(rendered.contains("\\f"));
    }

    #[test]
    fn render_string_escapes_control_char_as_unicode() {
        // U+0001 (SOH) — not backspace/formfeed/tab/newline/carriage-return
        let s = "\u{0001}";
        let rendered = render_string(s);
        assert!(rendered.contains("\\u0001"));
    }

    // --- round-trip tests ---

    #[test]
    fn round_trip_null() {
        let original = JsonValue::Null;
        let parsed = JsonValue::parse(&original.render()).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn round_trip_complex_object() {
        let mut map = BTreeMap::new();
        map.insert("num".to_string(), JsonValue::Number(-42));
        map.insert("flag".to_string(), JsonValue::Bool(false));
        map.insert(
            "text".to_string(),
            JsonValue::String("hello\nworld".to_string()),
        );
        map.insert("null_val".to_string(), JsonValue::Null);
        let original = JsonValue::Object(map);
        let rendered = original.render();
        let parsed = JsonValue::parse(&rendered).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn parse_array_with_whitespace() {
        let v = JsonValue::parse("[ 1 , 2 , 3 ]").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 3);
    }

    #[test]
    fn parse_escape_in_end_of_input() {
        // String terminated by backslash before closing quote
        let result = JsonValue::parse("\"\\");
        assert!(result.is_err());
    }
}
