use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(Vec<(Vec<u8>, Value)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    UnexpectedEof,
    InvalidPrefix(u8),
    InvalidInt,
    InvalidLen,
    TrailingData,
    InvalidDictKey,
    InvalidDictOrder,
    DepthLimitExceeded,
    ValueLimitExceeded,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEof => write!(f, "unexpected end of input"),
            Error::InvalidPrefix(b) => write!(f, "invalid prefix byte: 0x{b:02x}"),
            Error::InvalidInt => write!(f, "invalid integer"),
            Error::InvalidLen => write!(f, "invalid byte string length"),
            Error::TrailingData => write!(f, "trailing data"),
            Error::InvalidDictKey => write!(f, "invalid dict key"),
            Error::InvalidDictOrder => {
                write!(f, "dictionary keys are not in strictly increasing order")
            }
            Error::DepthLimitExceeded => write!(f, "bencode nesting limit exceeded"),
            Error::ValueLimitExceeded => write!(f, "bencode value count limit exceeded"),
        }
    }
}

impl std::error::Error for Error {}

pub fn parse(data: &[u8]) -> Result<Value, Error> {
    let (value, pos) = parse_value(data, 0)?;
    if pos != data.len() {
        return Err(Error::TrailingData);
    }
    Ok(value)
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

pub fn encode_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Int(num) => {
            out.push(b'i');
            if *num < 0 {
                out.push(b'-');
            }
            put_decimal(out, num.unsigned_abs());
            out.push(b'e');
        }
        Value::Bytes(bytes) => put_bytes(out, bytes),
        Value::List(items) => {
            out.push(b'l');
            for item in items {
                encode_into(item, out);
            }
            out.push(b'e');
        }
        Value::Dict(items) => {
            out.push(b'd');
            let already_sorted = items.windows(2).all(|w| w[0].0 <= w[1].0);
            let mut sorted_storage;
            let ordered: &[(Vec<u8>, Value)] = if already_sorted {
                items
            } else {
                sorted_storage = items.clone();
                sorted_storage.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                &sorted_storage
            };
            for (key, value) in ordered {
                put_bytes(out, key);
                encode_into(value, out);
            }
            out.push(b'e');
        }
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_decimal(out, bytes.len() as u64);
    out.push(b':');
    out.extend_from_slice(bytes);
}

/// Appends `value` in decimal without a temporary `String`.
fn put_decimal(out: &mut Vec<u8>, mut value: u64) {
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[start..]);
}

/// Verifies that an in-memory value fits the same depth and value-count
/// budgets enforced by the decoder. Dictionary keys count as byte-string
/// values because the decoder accounts for them independently.
pub fn validate_structure(value: &Value) -> Result<(), Error> {
    let mut remaining = MAX_VALUES;
    validate_structure_with_depth(value, 0, &mut remaining)
}

fn validate_structure_with_depth(
    value: &Value,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), Error> {
    consume_structure_slot(depth, remaining)?;
    match value {
        Value::Int(_) | Value::Bytes(_) => Ok(()),
        Value::List(items) => {
            for item in items {
                validate_structure_with_depth(item, depth + 1, remaining)?;
            }
            Ok(())
        }
        Value::Dict(items) => {
            for (_, value) in items {
                consume_structure_slot(depth + 1, remaining)?;
                validate_structure_with_depth(value, depth + 1, remaining)?;
            }
            Ok(())
        }
    }
}

fn consume_structure_slot(depth: usize, remaining: &mut usize) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::DepthLimitExceeded);
    }
    if *remaining == 0 {
        return Err(Error::ValueLimitExceeded);
    }
    *remaining -= 1;
    Ok(())
}

pub fn parse_value(data: &[u8], pos: usize) -> Result<(Value, usize), Error> {
    let mut remaining = MAX_VALUES;
    parse_value_with_budget(data, pos, &mut remaining)
}

/// Like `parse_value`, but charges a caller-owned value budget so several
/// consecutive values together stay within `MAX_VALUES`.
pub fn parse_value_with_budget(
    data: &[u8],
    pos: usize,
    remaining: &mut usize,
) -> Result<(Value, usize), Error> {
    parse_value_with_depth(data, pos, 0, remaining)
}

// A malicious metainfo or tracker response can otherwise exhaust the call stack
// with a few thousand nested lists or dictionaries. This is deliberately much
// higher than any practical torrent metadata needs.
const MAX_DEPTH: usize = 512;
// Small scalar values can otherwise expand a bounded input into millions of
// heap allocations. The limit is comfortably above legitimate metainfo,
// tracker, resume, and session structures.
pub(crate) const MAX_VALUES: usize = 262_144;

fn parse_value_with_depth(
    data: &[u8],
    pos: usize,
    depth: usize,
    remaining: &mut usize,
) -> Result<(Value, usize), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::DepthLimitExceeded);
    }
    if *remaining == 0 {
        return Err(Error::ValueLimitExceeded);
    }
    *remaining -= 1;
    if pos >= data.len() {
        return Err(Error::UnexpectedEof);
    }
    match data[pos] {
        b'i' => {
            let (value, next) = parse_int(data, pos)?;
            Ok((Value::Int(value), next))
        }
        b'l' => {
            let mut items = Vec::new();
            let mut i = pos + 1;
            while i < data.len() && data[i] != b'e' {
                let (value, next) = parse_value_with_depth(data, i, depth + 1, remaining)?;
                items.push(value);
                i = next;
            }
            if i >= data.len() {
                return Err(Error::UnexpectedEof);
            }
            Ok((Value::List(items), i + 1))
        }
        b'd' => {
            let mut items: Vec<(Vec<u8>, Value)> = Vec::new();
            let mut i = pos + 1;
            while i < data.len() && data[i] != b'e' {
                let (key_value, next) = parse_value_with_depth(data, i, depth + 1, remaining)?;
                let Value::Bytes(key) = key_value else {
                    return Err(Error::InvalidDictKey);
                };
                if items.last().is_some_and(|(previous, _)| *previous >= key) {
                    return Err(Error::InvalidDictOrder);
                }
                let (value, next) = parse_value_with_depth(data, next, depth + 1, remaining)?;
                items.push((key, value));
                i = next;
            }
            if i >= data.len() {
                return Err(Error::UnexpectedEof);
            }
            Ok((Value::Dict(items), i + 1))
        }
        b'0'..=b'9' => {
            let (bytes, next) = parse_bytes(data, pos)?;
            Ok((Value::Bytes(bytes), next))
        }
        other => Err(Error::InvalidPrefix(other)),
    }
}

fn parse_int(data: &[u8], pos: usize) -> Result<(i64, usize), Error> {
    let mut i = pos + 1;
    while i < data.len() && data[i] != b'e' {
        i += 1;
    }
    if i >= data.len() {
        return Err(Error::UnexpectedEof);
    }
    let slice = &data[pos + 1..i];
    if slice.is_empty() {
        return Err(Error::InvalidInt);
    }
    let valid_syntax = match slice {
        b"0" => true,
        [b'1'..=b'9', rest @ ..] => rest.iter().all(u8::is_ascii_digit),
        [b'-', b'1'..=b'9', rest @ ..] => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    };
    if !valid_syntax {
        return Err(Error::InvalidInt);
    }
    let (negative, digits) = match slice {
        [b'-', rest @ ..] => (true, rest),
        _ => (false, slice),
    };
    // Accumulate negatively so i64::MIN is representable.
    let mut value = 0i64;
    for digit in digits {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_sub(i64::from(digit - b'0')))
            .ok_or(Error::InvalidInt)?;
    }
    if !negative {
        value = value.checked_neg().ok_or(Error::InvalidInt)?;
    }
    Ok((value, i + 1))
}

fn parse_bytes(data: &[u8], pos: usize) -> Result<(Vec<u8>, usize), Error> {
    let mut i = pos;
    while i < data.len() && is_digit(data[i]) {
        i += 1;
    }
    if i == pos || i >= data.len() || data[i] != b':' {
        return Err(Error::InvalidLen);
    }
    let slice = &data[pos..i];
    if slice.len() > 1 && slice[0] == b'0' {
        return Err(Error::InvalidLen);
    }
    let mut len = 0usize;
    for digit in slice {
        len = len
            .checked_mul(10)
            .and_then(|len| len.checked_add(usize::from(digit - b'0')))
            .ok_or(Error::InvalidLen)?;
    }
    let start = i + 1;
    let end = start.checked_add(len).ok_or(Error::InvalidLen)?;
    if end > data.len() {
        return Err(Error::UnexpectedEof);
    }
    Ok((data[start..end].to_vec(), end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dict() {
        let value = Value::Dict(vec![
            (b"bar".to_vec(), Value::Int(42)),
            (
                b"foo".to_vec(),
                Value::List(vec![Value::Bytes(b"hi".to_vec())]),
            ),
        ]);
        let encoded = encode(&value);
        let decoded = parse(&encoded).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn rejects_negative_zero() {
        assert!(parse(b"i-0e").is_err());
    }

    #[test]
    fn rejects_trailing_data() {
        assert!(matches!(parse(b"i1ee"), Err(Error::TrailingData)));
    }

    #[test]
    fn rejects_invalid_dict_key_type() {
        assert!(matches!(parse(b"di1e1:ae"), Err(Error::InvalidDictKey)));
    }

    #[test]
    fn rejects_invalid_lengths_and_integers() {
        assert!(matches!(parse(b"03:abc"), Err(Error::InvalidLen)));
        assert!(matches!(parse(b"i01e"), Err(Error::InvalidInt)));
        assert!(matches!(parse(b"ie"), Err(Error::InvalidInt)));
        assert!(matches!(parse(b"i+1e"), Err(Error::InvalidInt)));

        let overflowing_end = format!("{}:x", usize::MAX);
        assert!(matches!(
            parse(overflowing_end.as_bytes()),
            Err(Error::InvalidLen)
        ));
    }

    #[test]
    fn rejects_noncanonical_dictionary_order_and_duplicates() {
        assert!(matches!(
            parse(b"d1:bi1e1:ai2ee"),
            Err(Error::InvalidDictOrder)
        ));
        assert!(matches!(
            parse(b"d1:ai1e1:ai2ee"),
            Err(Error::InvalidDictOrder)
        ));
    }

    #[test]
    fn rejects_excessive_nesting_without_overflowing_the_stack() {
        let mut data = vec![b'l'; MAX_DEPTH + 2];
        data.extend(std::iter::repeat_n(b'e', MAX_DEPTH + 2));
        assert!(matches!(parse(&data), Err(Error::DepthLimitExceeded)));
    }

    #[test]
    fn rejects_structures_that_exceed_the_value_budget() {
        let mut remaining = 2;
        assert!(matches!(
            parse_value_with_depth(b"li0ei1ee", 0, 0, &mut remaining),
            Err(Error::ValueLimitExceeded)
        ));
    }

    #[test]
    fn structure_validator_matches_decoder_value_accounting() {
        let one_pair = Value::Dict(vec![(b"key".to_vec(), Value::Int(0))]);
        let mut validator_remaining = 2;
        assert!(matches!(
            validate_structure_with_depth(&one_pair, 0, &mut validator_remaining),
            Err(Error::ValueLimitExceeded)
        ));
        let mut decoder_remaining = 2;
        assert!(matches!(
            parse_value_with_depth(b"d3:keyi0ee", 0, 0, &mut decoder_remaining),
            Err(Error::ValueLimitExceeded)
        ));

        let at_limit = Value::List(vec![Value::Int(0); MAX_VALUES - 1]);
        validate_structure(&at_limit).unwrap();
        assert!(parse(&encode(&at_limit)).is_ok());

        let over_limit = Value::List(vec![Value::Int(0); MAX_VALUES]);
        assert!(matches!(
            validate_structure(&over_limit),
            Err(Error::ValueLimitExceeded)
        ));
        assert!(matches!(
            parse(&encode(&over_limit)),
            Err(Error::ValueLimitExceeded)
        ));

        let mut too_deep = Value::Int(0);
        for _ in 0..=MAX_DEPTH {
            too_deep = Value::List(vec![too_deep]);
        }
        assert!(matches!(
            validate_structure(&too_deep),
            Err(Error::DepthLimitExceeded)
        ));
        assert!(matches!(
            parse(&encode(&too_deep)),
            Err(Error::DepthLimitExceeded)
        ));
    }

    #[test]
    fn integers_cover_the_full_i64_range() {
        for value in [0, 1, -1, 42, i64::MAX, i64::MIN] {
            let encoded = encode(&Value::Int(value));
            assert_eq!(encoded, format!("i{value}e").into_bytes());
            assert_eq!(parse(&encoded), Ok(Value::Int(value)));
        }
        assert_eq!(parse(b"i9223372036854775808e"), Err(Error::InvalidInt));
        assert_eq!(parse(b"i-9223372036854775809e"), Err(Error::InvalidInt));
        assert_eq!(
            encode(&Value::Bytes(vec![b'x'; 12])),
            b"12:xxxxxxxxxxxx".to_vec()
        );
    }

    #[test]
    fn shared_budget_limits_consecutive_values() {
        let mut remaining = 3;
        assert!(parse_value_with_budget(b"li1ee", 0, &mut remaining).is_ok());
        assert_eq!(remaining, 1);
        assert_eq!(
            parse_value_with_budget(b"li1ee", 0, &mut remaining),
            Err(Error::ValueLimitExceeded)
        );
    }

    #[test]
    fn parse_value_reports_next_offset() {
        let data = b"4:spami42e";
        let (first, pos) = parse_value(data, 0).unwrap();
        assert_eq!(first, Value::Bytes(b"spam".to_vec()));
        let (second, end) = parse_value(data, pos).unwrap();
        assert_eq!(second, Value::Int(42));
        assert_eq!(end, data.len());
    }
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parse_integer() {
        let value = parse(b"i42e").unwrap();
        assert_eq!(value, Value::Int(42));
    }

    #[test]
    fn parse_bytes() {
        let value = parse(b"4:spam").unwrap();
        assert_eq!(value, Value::Bytes(b"spam".to_vec()));
    }

    #[test]
    fn parse_list_and_dict() {
        let value = parse(b"l4:spam4:eggse").unwrap();
        assert_eq!(
            value,
            Value::List(vec![
                Value::Bytes(b"spam".to_vec()),
                Value::Bytes(b"eggs".to_vec())
            ])
        );

        let value = parse(b"d3:cow3:moo4:spam4:eggse").unwrap();
        assert_eq!(
            value,
            Value::Dict(vec![
                (b"cow".to_vec(), Value::Bytes(b"moo".to_vec())),
                (b"spam".to_vec(), Value::Bytes(b"eggs".to_vec()))
            ])
        );
    }
}
