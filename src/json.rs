use serde_json::{Map, Number, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidJson;

/// Parses JSON text into a `Value`, accepting nesting depths that exceed the
/// recursion limit of the default `serde_json` parser.
pub fn parse_value(bytes: &[u8]) -> Result<Value, InvalidJson> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        return Ok(value);
    }
    fallback_parse(bytes).ok_or(InvalidJson)
}

/// Serializes a `Value` to compact JSON text without recursion, so deeply
/// nested values that the parser accepts cannot overflow the stack.
pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    append_value(value, &mut out);
    out
}

/// Appends the compact JSON serialization of `value` to `out` without recursion.
pub fn append_value(value: &Value, out: &mut String) {
    write_value(out, value);
}

/// Appends `text` as a quoted, escaped JSON string literal to `out`.
pub fn append_string(out: &mut String, text: &str) {
    write_string(out, text);
}

enum PendingItem<'a> {
    Value(&'a Value),
    Keyed(&'a str, &'a Value),
}

enum WriteFrame<'a> {
    Object {
        entries: Vec<(&'a str, &'a Value)>,
        next: usize,
    },
    Array {
        items: &'a [Value],
        next: usize,
    },
}

fn write_value<'a>(out: &mut String, root: &'a Value) {
    let mut stack: Vec<WriteFrame<'a>> = Vec::new();
    let mut pending: Option<PendingItem<'a>> = Some(PendingItem::Value(root));

    loop {
        let item = match pending.take() {
            Some(item) => item,
            None => match next_item(out, &mut stack) {
                Some(item) => item,
                None => {
                    if let Some(frame) = stack.pop() {
                        out.push(match frame {
                            WriteFrame::Object { .. } => '}',
                            WriteFrame::Array { .. } => ']',
                        });
                        continue;
                    }
                    break;
                }
            },
        };

        let (key, value) = match item {
            PendingItem::Value(value) => (None, value),
            PendingItem::Keyed(key, value) => (Some(key), value),
        };

        if let Some(key) = key {
            write_string(out, key);
            out.push(':');
        }

        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(number) => out.push_str(&number.to_string()),
            Value::String(text) => write_string(out, text),
            Value::Array(items) if items.is_empty() => out.push_str("[]"),
            Value::Object(map) if map.is_empty() => out.push_str("{}"),
            Value::Array(items) => {
                let first = &items[0];
                out.push('[');
                stack.push(WriteFrame::Array {
                    items: items.as_slice(),
                    next: 0,
                });
                pending = Some(PendingItem::Value(first));
            }
            Value::Object(map) => {
                let entries: Vec<(&str, &Value)> = map
                    .iter()
                    .map(|(key, value)| (key.as_str(), value))
                    .collect();
                let (first_key, first_value) = entries[0];
                out.push('{');
                stack.push(WriteFrame::Object { entries, next: 0 });
                pending = Some(PendingItem::Keyed(first_key, first_value));
            }
        }
    }
}

fn next_item<'a>(out: &mut String, stack: &mut Vec<WriteFrame<'a>>) -> Option<PendingItem<'a>> {
    let frame = stack.last_mut()?;
    match frame {
        WriteFrame::Object { entries, next } => {
            *next += 1;
            if *next < entries.len() {
                out.push(',');
                Some(PendingItem::Keyed(entries[*next].0, entries[*next].1))
            } else {
                None
            }
        }
        WriteFrame::Array { items, next } => {
            *next += 1;
            if *next < items.len() {
                out.push(',');
                Some(PendingItem::Value(&items[*next]))
            } else {
                None
            }
        }
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                let code = ch as u32;
                out.push('\\');
                out.push('u');
                for shift in [12u32, 8, 4, 0] {
                    let digit = (code >> shift) & 0xF;
                    out.push(char::from_digit(digit, 16).expect("hex digit"));
                }
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.bytes.get(self.pos) {
            match b {
                0x20 | 0x09 | 0x0A | 0x0D => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) {
        if self.pos < self.bytes.len() {
            self.pos += 1;
        }
    }
}

#[derive(Clone, Copy)]
enum Step {
    Value,
    ObjKeyOrEnd,
    ObjKeyAfterComma,
    ObjColon,
    ArrValueOrEnd,
    ArrValueAfterComma,
    ObjCommaOrEnd,
    ArrCommaOrEnd,
}

enum Frame {
    Object {
        map: Map<String, Value>,
        pending_key: Option<String>,
    },
    Array {
        items: Vec<Value>,
    },
}

enum Outcome {
    Done(Value),
    Next(Step),
}

fn deliver(stack: &mut [Frame], value: Value) -> Outcome {
    match stack.last_mut() {
        None => Outcome::Done(value),
        Some(Frame::Object { map, pending_key }) => {
            if let Some(key) = pending_key.take() {
                map.insert(key, value);
            }
            Outcome::Next(Step::ObjCommaOrEnd)
        }
        Some(Frame::Array { items }) => {
            items.push(value);
            Outcome::Next(Step::ArrCommaOrEnd)
        }
    }
}

fn close_frame(stack: &mut Vec<Frame>) -> Option<Value> {
    match stack.pop()? {
        Frame::Object { map, pending_key } => {
            if pending_key.is_some() {
                return None;
            }
            Some(Value::Object(map))
        }
        Frame::Array { items } => Some(Value::Array(items)),
    }
}

fn finish(p: &mut Parser, value: Value) -> Option<Value> {
    p.skip_ws();
    (p.pos == p.bytes.len()).then_some(value)
}

fn open_container(p: &mut Parser, stack: &mut Vec<Frame>, b: u8) -> Step {
    p.bump();
    if b == b'{' {
        stack.push(Frame::Object {
            map: Map::new(),
            pending_key: None,
        });
        Step::ObjKeyOrEnd
    } else {
        stack.push(Frame::Array { items: Vec::new() });
        Step::ArrValueOrEnd
    }
}

fn fallback_parse(bytes: &[u8]) -> Option<Value> {
    let mut p = Parser { bytes, pos: 0 };
    p.skip_ws();
    p.peek()?;

    let mut stack: Vec<Frame> = Vec::new();
    let mut step = Step::Value;

    loop {
        match step {
            Step::Value => {
                p.skip_ws();
                let b = p.peek()?;
                if b == b'{' || b == b'[' {
                    step = open_container(&mut p, &mut stack, b);
                } else {
                    let value = parse_scalar(&mut p)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                }
            }

            Step::ObjKeyOrEnd => {
                p.skip_ws();
                let b = p.peek()?;
                if b == b'}' {
                    p.bump();
                    let value = close_frame(&mut stack)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                } else if b == b'"' {
                    let key = parse_string(&mut p)?;
                    match stack.last_mut() {
                        Some(Frame::Object { pending_key, .. }) => *pending_key = Some(key),
                        _ => return None,
                    }
                    step = Step::ObjColon;
                } else {
                    return None;
                }
            }

            Step::ObjKeyAfterComma => {
                p.skip_ws();
                if p.peek()? != b'"' {
                    return None;
                }
                let key = parse_string(&mut p)?;
                match stack.last_mut() {
                    Some(Frame::Object { pending_key, .. }) => *pending_key = Some(key),
                    _ => return None,
                }
                step = Step::ObjColon;
            }

            Step::ObjColon => {
                p.skip_ws();
                if p.peek()? != b':' {
                    return None;
                }
                p.bump();
                step = Step::Value;
            }

            Step::ArrValueOrEnd => {
                p.skip_ws();
                let b = p.peek()?;
                if b == b']' {
                    p.bump();
                    let value = close_frame(&mut stack)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                } else if b == b'{' || b == b'[' {
                    step = open_container(&mut p, &mut stack, b);
                } else {
                    let value = parse_scalar(&mut p)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                }
            }

            Step::ObjCommaOrEnd => {
                p.skip_ws();
                let b = p.peek()?;
                if b == b'}' {
                    p.bump();
                    let value = close_frame(&mut stack)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                } else if b == b',' {
                    p.bump();
                    step = Step::ObjKeyAfterComma;
                } else {
                    return None;
                }
            }

            Step::ArrValueAfterComma => {
                p.skip_ws();
                let b = p.peek()?;
                if b == b'{' || b == b'[' {
                    step = open_container(&mut p, &mut stack, b);
                } else {
                    let value = parse_scalar(&mut p)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                }
            }

            Step::ArrCommaOrEnd => {
                p.skip_ws();
                let b = p.peek()?;
                if b == b']' {
                    p.bump();
                    let value = close_frame(&mut stack)?;
                    match deliver(&mut stack, value) {
                        Outcome::Done(v) => return finish(&mut p, v),
                        Outcome::Next(next) => step = next,
                    }
                } else if b == b',' {
                    p.bump();
                    step = Step::ArrValueAfterComma;
                } else {
                    return None;
                }
            }
        }
    }
}

fn parse_scalar(p: &mut Parser) -> Option<Value> {
    let b = p.peek()?;
    match b {
        b'"' => Some(Value::String(parse_string(p)?)),
        b't' => expect_literal(p, b"true").map(|_| Value::Bool(true)),
        b'f' => expect_literal(p, b"false").map(|_| Value::Bool(false)),
        b'n' => expect_literal(p, b"null").map(|_| Value::Null),
        _ => parse_number(p).map(Value::Number),
    }
}

fn expect_literal(p: &mut Parser, literal: &[u8]) -> Option<()> {
    let start = p.pos;
    for expected in literal {
        if p.peek()? != *expected {
            return None;
        }
        p.bump();
    }
    (start + literal.len() == p.pos).then_some(())
}

fn parse_number(p: &mut Parser) -> Option<Number> {
    let start = p.pos;
    if p.peek()? == b'-' {
        p.bump();
    }
    match p.peek()? {
        b'0' => {
            p.bump();
            if matches!(p.peek(), Some(b'0'..=b'9')) {
                return None;
            }
        }
        b'1'..=b'9' => {
            while matches!(p.peek(), Some(b'0'..=b'9')) {
                p.bump();
            }
        }
        _ => return None,
    }
    if p.peek()? == b'.' {
        p.bump();
        if !matches!(p.peek(), Some(b'0'..=b'9')) {
            return None;
        }
        while matches!(p.peek(), Some(b'0'..=b'9')) {
            p.bump();
        }
    }
    if matches!(p.peek(), Some(b'e') | Some(b'E')) {
        p.bump();
        if matches!(p.peek(), Some(b'+') | Some(b'-')) {
            p.bump();
        }
        if !matches!(p.peek(), Some(b'0'..=b'9')) {
            return None;
        }
        while matches!(p.peek(), Some(b'0'..=b'9')) {
            p.bump();
        }
    }

    let token = &p.bytes[start..p.pos];
    serde_json::from_slice::<Number>(token).ok()
}

fn parse_string(p: &mut Parser) -> Option<String> {
    if p.peek()? != b'"' {
        return None;
    }
    p.bump();

    let mut out = Vec::new();
    loop {
        let b = p.peek()?;
        match b {
            b'"' => {
                p.bump();
                return String::from_utf8(out).ok();
            }
            b'\\' => {
                p.bump();
                let esc = p.peek()?;
                p.bump();
                match esc {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0C),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let hi = parse_hex4(p)?;
                        let cp = if (0xD800..0xDC00).contains(&hi) {
                            if p.peek()? != b'\\' || *p.bytes.get(p.pos + 1)? != b'u' {
                                return None;
                            }
                            p.bump();
                            p.bump();
                            let lo = parse_hex4(p)?;
                            if !(0xDC00..0xE000).contains(&lo) {
                                return None;
                            }
                            0x10000 + (((hi - 0xD800) as u32) << 10) + (lo - 0xDC00) as u32
                        } else if (0xDC00..0xE000).contains(&hi) {
                            return None;
                        } else {
                            hi as u32
                        };
                        let ch = char::from_u32(cp)?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return None,
                }
            }
            0x00..=0x1F => return None,
            _ => {
                let len = utf8_len(b)?;
                if p.bytes.len() - p.pos < len {
                    return None;
                }
                for i in 1..len {
                    if !matches!(p.bytes[p.pos + i], 0x80..=0xBF) {
                        return None;
                    }
                }
                let slice = &p.bytes[p.pos..p.pos + len];
                if std::str::from_utf8(slice).is_err() {
                    return None;
                }
                out.extend_from_slice(slice);
                p.pos += len;
            }
        }
    }
}

fn parse_hex4(p: &mut Parser) -> Option<u16> {
    let mut value: u16 = 0;
    for _ in 0..4 {
        let b = p.peek()?;
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u16,
            b'a'..=b'f' => (b - b'a' + 10) as u16,
            b'A'..=b'F' => (b - b'A' + 10) as u16,
            _ => return None,
        };
        value = value.checked_shl(4)?.checked_add(digit)?;
        p.bump();
    }
    Some(value)
}

fn utf8_len(b: u8) -> Option<usize> {
    if b < 0x80 {
        Some(1)
    } else if (0xC2..=0xDF).contains(&b) {
        Some(2)
    } else if (0xE0..=0xEF).contains(&b) {
        Some(3)
    } else if (0xF0..=0xF4).contains(&b) {
        Some(4)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deep_array(depth: usize) -> String {
        let mut s = String::with_capacity(depth * 2);
        for _ in 0..depth {
            s.push('[');
        }
        for _ in 0..depth {
            s.push(']');
        }
        s
    }

    fn deep_object(depth: usize) -> String {
        let mut s = String::new();
        for _ in 0..depth - 1 {
            s.push_str("{\"a\":");
        }
        s.push_str("{}");
        for _ in 0..depth - 1 {
            s.push('}');
        }
        s
    }

    #[test]
    fn parses_scalars_and_simple_structures() {
        assert_eq!(parse_value(b"null").unwrap(), Value::Null);
        assert_eq!(parse_value(b"true").unwrap(), Value::Bool(true));
        assert_eq!(parse_value(b"false").unwrap(), Value::Bool(false));
        assert_eq!(parse_value(b"42").unwrap(), json_number("42"));
        assert_eq!(parse_value(b"-1.5e+3").unwrap(), json_number("-1.5e+3"));
        assert_eq!(
            parse_value(b"\"hi\"").unwrap(),
            Value::String("hi".to_owned())
        );
        assert_eq!(parse_value(b"[1, 2]").unwrap(), serde_json::json!([1, 2]));
        assert_eq!(
            parse_value(b"{ \"a\": [true, null] }").unwrap(),
            serde_json::json!({ "a": [true, null] })
        );
    }

    fn json_number(text: &str) -> Value {
        serde_json::from_str::<Value>(text).expect("valid number literal")
    }

    #[test]
    fn parses_deeply_nested_arrays_beyond_serde_limit() {
        let text = deep_array(200);
        let value = parse_value(text.as_bytes()).unwrap();
        let mut current = &value;
        for _ in 0..199 {
            let items = current.as_array().expect("nested array");
            assert_eq!(items.len(), 1);
            current = &items[0];
        }
        assert!(current.as_array().expect("innermost array").is_empty());
    }

    #[test]
    fn parses_deeply_nested_objects_beyond_serde_limit() {
        let text = deep_object(200);
        let value = parse_value(text.as_bytes()).unwrap();
        let mut current = &value;
        for _ in 0..199 {
            let obj = current.as_object().expect("nested object");
            assert_eq!(obj.len(), 1);
            current = &obj["a"];
        }
        assert!(current.as_object().expect("innermost object").is_empty());
    }

    #[test]
    fn parses_mixed_deep_nesting() {
        let mut text = String::new();
        for _ in 0..150 {
            text.push_str("{\"k\":[");
        }
        text.push('7');
        for _ in 0..150 {
            text.push_str("]}");
        }
        let value = parse_value(text.as_bytes()).unwrap();
        let mut current = &value;
        for _ in 0..150 {
            let obj = current.as_object().expect("nested object");
            let arr = obj
                .get("k")
                .and_then(Value::as_array)
                .expect("nested array");
            assert_eq!(arr.len(), 1);
            current = &arr[0];
        }
        assert_eq!(current, &json_number("7"));
    }

    #[test]
    fn parses_string_escapes_and_surrogate_pairs() {
        let text = "\"a\\nb\\u00e9\\ud83d\\ude00\"";
        let value = parse_value(text.as_bytes()).unwrap();
        assert_eq!(value, Value::String("a\nb\u{e9}\u{1F600}".to_owned()));

        let raw = "\"caf\u{e9}\"".as_bytes().to_vec();
        assert_eq!(
            parse_value(&raw).unwrap(),
            Value::String("caf\u{e9}".to_owned())
        );
    }

    #[test]
    fn rejects_uppercase_u_in_surrogate_pair_continuation() {
        let text = r#""\ud83d\Ude00""#;
        assert_eq!(parse_value(text.as_bytes()), Err(InvalidJson));
    }

    #[test]
    fn rejects_invalid_documents() {
        for text in [
            "",
            "   ",
            "{",
            "[1,",
            "[1,]",
            "{\"a\"}",
            "{\"a\":1,}",
            "[1 2]",
            "01",
            "- 1",
            "1.",
            "1e",
            "\"\\x41\"",
            "\"\u{1}\"",
            "\"truncated",
            "tru",
            "nul",
            "true false",
            "[1] extra",
        ] {
            assert_eq!(
                parse_value(text.as_bytes()),
                Err(InvalidJson),
                "accepted: {text}"
            );
        }
    }

    #[test]
    fn rejects_invalid_utf8_in_strings() {
        let mut bytes = b"\"ab".to_vec();
        bytes.push(0xFF);
        assert_eq!(parse_value(&bytes), Err(InvalidJson));
    }

    #[test]
    fn preserves_big_integers_via_number_delegation() {
        let text = r#"{"n":18446744073709551617}"#;
        let value = parse_value(text.as_bytes()).unwrap();
        assert_eq!(value, serde_json::from_str::<Value>(text).expect("valid"));
    }

    #[test]
    fn duplicate_object_keys_last_wins() {
        let value = parse_value(b"{\"a\":1,\"a\":2}").unwrap();
        assert_eq!(value, serde_json::json!({ "a": 2 }));
    }

    #[test]
    fn serializes_scalars_and_simple_structures() {
        let cases = [
            (Value::Null, "null"),
            (Value::Bool(true), "true"),
            (Value::Bool(false), "false"),
            (json_number("42"), "42"),
            (serde_json::json!([]), "[]"),
            (serde_json::json!({}), "{}"),
            (serde_json::json!([1, 2]), "[1,2]"),
            (
                serde_json::json!({"a": [true, null]}),
                "{\"a\":[true,null]}",
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(to_string(&value), expected);
        }
    }

    #[test]
    fn serializes_numbers_like_serde_json() {
        for text in [
            "42",
            "-7",
            "0",
            "-0",
            "3.14",
            "-1.5e+3",
            "1E5",
            "0.1",
            "18446744073709551617",
            "1e400",
            "-1e400",
        ] {
            let value = json_number(text);
            assert_eq!(to_string(&value), serde_json::to_string(&value).unwrap());
        }
    }

    #[test]
    fn serializes_object_keys_in_sorted_order() {
        let value = serde_json::json!({"b": 1, "a": 2, "c": [3]});
        assert_eq!(to_string(&value), "{\"a\":2,\"b\":1,\"c\":[3]}");
    }

    #[test]
    fn serializes_string_escapes_like_serde_json() {
        assert_eq!(to_string(&Value::String("a\"b".into())), "\"a\\\"b\"");
        assert_eq!(to_string(&Value::String("a\\b".into())), "\"a\\\\b\"");
        assert_eq!(to_string(&Value::String("\u{1}".into())), "\"\\u0001\"");
        assert_eq!(
            to_string(&Value::String("\n\r\t\u{8}\u{c}".into())),
            "\"\\n\\r\\t\\b\\f\""
        );

        let text =
            "quote\" backslash\\ tab\t nl\n cr\r bs\u{8} ff\u{c} ctrl\u{1} caf\u{e9} \u{1F600}";
        let value = Value::String(text.to_owned());
        assert_eq!(to_string(&value), serde_json::to_string(&value).unwrap());
    }

    fn run_on_large_stack(f: impl FnOnce() -> String + Send + 'static) -> String {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(f)
            .expect("spawn large-stack test thread")
            .join()
            .expect("large-stack test thread panicked")
    }

    #[test]
    fn serializes_deeply_nested_arrays_beyond_serde_limit() {
        let text = deep_array(50_000);
        let input = text.clone();
        let out = run_on_large_stack(move || {
            let value = parse_value(input.as_bytes()).unwrap();
            to_string(&value)
        });
        assert_eq!(out, text);
    }

    #[test]
    fn serializes_deeply_nested_objects_beyond_serde_limit() {
        let text = deep_object(50_000);
        let input = text.clone();
        let out = run_on_large_stack(move || {
            let value = parse_value(input.as_bytes()).unwrap();
            to_string(&value)
        });
        assert_eq!(out, text);
    }

    #[test]
    fn serializes_mixed_deep_nesting_beyond_serde_limit() {
        let mut text = String::new();
        for _ in 0..25_000 {
            text.push_str("{\"k\":[");
        }
        text.push('7');
        for _ in 0..25_000 {
            text.push_str("]}");
        }
        let input = text.clone();
        let out = run_on_large_stack(move || {
            let value = parse_value(input.as_bytes()).unwrap();
            to_string(&value)
        });
        assert_eq!(out, text);
    }
}
