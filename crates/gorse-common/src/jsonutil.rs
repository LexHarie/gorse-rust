// Ports: common/jsonutil/json.go @ add21b5

//! JSON helpers that match Go's `encoding/json` for the values Gorse stores.
//!
//! [`marshal`] writes the bytes Go's `json.Marshal` writes: HTML characters
//! escaped as `<`, `>` and `&`, U+2028 and U+2029 escaped,
//! `\b` and `\f` as short escapes, floats in Go's format, and
//! non-finite floats rejected. [`unmarshal`] reads JSON as `json.Unmarshal`
//! does into a fresh value, with empty input and `null` giving the default.

use std::io::{self, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::ser::{CharEscape, Formatter};

/// Errors from [`marshal`] and [`unmarshal`]; `serde_json` reports both.
pub type Error = serde_json::Error;

/// The serialiser formatter behind [`marshal`]: `serde_json`'s compact output,
/// with the escapes and number formats `encoding/json` uses.
#[derive(Debug, Default, Clone, Copy)]
struct GoFormatter;

impl Formatter for GoFormatter {
    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        char_escape: CharEscape,
    ) -> io::Result<()> {
        match char_escape {
            CharEscape::Quote => writer.write_all(b"\\\""),
            CharEscape::ReverseSolidus => writer.write_all(b"\\\\"),
            CharEscape::Solidus => writer.write_all(b"/"),
            CharEscape::Backspace => writer.write_all(b"\\b"),
            CharEscape::FormFeed => writer.write_all(b"\\f"),
            CharEscape::LineFeed => writer.write_all(b"\\n"),
            CharEscape::CarriageReturn => writer.write_all(b"\\r"),
            CharEscape::Tab => writer.write_all(b"\\t"),
            CharEscape::AsciiControl(byte) => write!(writer, "\\u{byte:04x}"),
        }
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (index, ch) in fragment.char_indices() {
            let escape = match ch {
                '<' => "\\u003c",
                '>' => "\\u003e",
                '&' => "\\u0026",
                '\u{2028}' => "\\u2028",
                '\u{2029}' => "\\u2029",
                _ => continue,
            };
            writer.write_all(&fragment.as_bytes()[start..index])?;
            writer.write_all(escape.as_bytes())?;
            start = index + ch.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(json_float(value).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        writer.write_all(json_float32(value).as_bytes())
    }
}

/// `encoding/json`'s float rule: shortest digits, exponent form outside
/// [1e-6, 1e21), with the exponent's sign always written and a leading zero
/// dropped (`1e+21`, `1e-7`).
fn json_float(value: f64) -> String {
    let abs = value.abs();
    go_float(
        value.is_sign_negative(),
        value == 0.0,
        abs != 0.0 && (abs < 1e-6 || abs >= 1e21),
        &format!("{value:e}"),
        |digits| format!("{value:.*e}", digits - 1),
    )
}

/// [`json_float`] for a `float32`, with its thresholds compared in `float32`.
fn json_float32(value: f32) -> String {
    let abs = value.abs();
    go_float(
        value.is_sign_negative(),
        value == 0.0,
        abs != 0.0 && (abs < 1e-6 || abs >= 1e21),
        &format!("{value:e}"),
        |digits| format!("{value:.*e}", digits - 1),
    )
}

/// Go's float text, from Rust's formatting. `shortest` is the shortest round-trip form
/// (`{:e}`), and `rounded(n)` is the value rounded to `n` significant digits (`{:.(n-1)e}`).
///
/// Rust and Go both write the shortest digit count, but when the exact value lies halfway
/// between two candidates of that count, Rust's shortest form rounds up and Go's rounds to
/// the even digit. Both candidates round-trip to the same float, so the bytes differ.
/// The digits are therefore taken from the exact rounding at Rust's shortest count, which
/// rounds ties to even, and the layout follows Go's `%f` or `%e` rule.
fn go_float(
    negative: bool,
    zero: bool,
    exponent: bool,
    shortest: &str,
    rounded: impl Fn(usize) -> String,
) -> String {
    if zero {
        return if negative { "-0" } else { "0" }.to_owned();
    }
    let count = shortest
        .split('e')
        .next()
        .unwrap_or_default()
        .trim_start_matches('-')
        .replace('.', "")
        .len();
    let exact = rounded(count);
    let (mantissa, exp) = exact.split_once('e').unwrap_or((&exact, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut digits = mantissa.trim_start_matches('-').replace('.', "");
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if exponent {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        if exp < 0 {
            out.push('-');
            out.push_str(&(-exp).to_string());
        } else {
            out.push('+');
            out.push_str(&exp.to_string());
        }
        return out;
    }
    let point = exp + 1;
    let len = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if point <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-point) as usize));
        out.push_str(&digits);
    } else if point >= len {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (point - len) as usize));
    } else {
        let split = point as usize;
        out.push_str(&digits[..split]);
        out.push('.');
        out.push_str(&digits[split..]);
    }
    out
}

/// Go's `json.Marshal`: the JSON encoding of `value`.
pub fn marshal<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, GoFormatter);
    value.serialize(&mut serializer)?;
    Ok(out)
}

/// Go's `jsonutil.MustMarshal`: [`marshal`] as a string.
///
/// # Panics
///
/// Panics when `value` cannot be encoded, as Go's `MustMarshal` does.
pub fn must_marshal<T: Serialize + ?Sized>(value: &T) -> String {
    match marshal(value) {
        Ok(data) => String::from_utf8(data).expect("serde_json writes UTF-8"),
        Err(err) => panic!("{err}"),
    }
}

/// Go's `jsonutil.Unmarshal`: parses `data` into `value`. Empty input is read
/// as `null`. Both `null` and empty input clear the value to `T::default()`,
/// as Go's `json.Unmarshal` does for the slices, maps and pointers Gorse reads.
pub fn unmarshal<T: DeserializeOwned + Default>(data: &[u8], value: &mut T) -> Result<(), Error> {
    let data: &[u8] = if data.is_empty() { b"null" } else { data };
    let parsed: Option<T> = serde_json::from_slice(data)?;
    *value = parsed.unwrap_or_default();
    Ok(())
}

#[cfg(test)]
mod tests {
    // Ports: common/jsonutil/json_test.go @ add21b5

    use super::*;

    #[test]
    fn unmarshal_slice() {
        let mut a: Vec<i64> = Vec::new();
        unmarshal(b"[1,2,3]", &mut a).expect("valid JSON");
        assert_eq!(a, vec![1, 2, 3]);
        // Empty input clears the value, as Go does.
        unmarshal(b"", &mut a).expect("empty input is null");
        assert!(a.is_empty());
    }

    #[test]
    fn marshal_nil() {
        let data = marshal(&None::<i64>).expect("null encodes");
        assert_eq!(data, b"null");
    }

    /// A value Go cannot encode, the analogue of the `chan` Go's test uses.
    struct Unsupported;

    impl Serialize for Unsupported {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("unsupported type: chan int"))
        }
    }

    #[test]
    #[should_panic(expected = "unsupported type: chan int")]
    fn must_marshal_panics_on_unsupported_value() {
        must_marshal(&Unsupported);
    }

    #[test]
    fn marshal_escapes_like_go() {
        assert_eq!(
            must_marshal("<a&b>\u{2028}\u{2029}\u{8}\u{c}\n\u{1}\"\\/é"),
            r#""\u003ca\u0026b\u003e\u2028\u2029\b\f\n\u0001\"\\/é""#
        );
    }

    #[test]
    fn marshal_floats_like_go() {
        assert_eq!(must_marshal(&1e21_f64), "1e+21");
        assert_eq!(must_marshal(&1e-7_f64), "1e-7");
        assert_eq!(must_marshal(&1.5e300_f64), "1.5e+300");
        assert_eq!(must_marshal(&0.000001_f64), "0.000001");
        assert_eq!(must_marshal(&100.0_f64), "100");
        assert_eq!(must_marshal(&-0.0_f64), "-0");
        assert_eq!(must_marshal(&1e21_f32), "1e+21");
        assert_eq!(must_marshal(&0.1_f32), "0.1");
    }

    /// Go's `json.Marshal(float64(float32(x)))` for values whose exact decimal form ends in
    /// a 5 at the 18th digit. Both strings of the tie round-trip to the same float64, and Go
    /// writes the one with the even last digit (`...562`). Rust's shortest formatting rounds
    /// the tie up (`...563`), so these values differ from Go's bytes.
    #[test]
    fn marshal_float_ties_round_to_even_like_go() {
        for text in [
            "-0.08755874633789062",
            "0.09210586547851562",
            "-0.07051467895507812",
            "-0.07471084594726562",
            "-0.09213638305664062",
            "0.026849746704101562",
            "0.027437210083007812",
        ] {
            let value: f64 = text.parse().expect("a float");
            assert_eq!(must_marshal(&value), text);
        }
    }

    /// The same for `float32`: Go's `json.Marshal(float32(x))` for float32 values with an exact
    /// tie at the shortest digit count (`1030.7812`, not Rust's `1030.7813`).
    #[test]
    fn marshal_float32_ties_round_to_even_like_go() {
        for text in ["1030.7812", "-4.8789062", "-1380.5312"] {
            let value: f32 = text.parse().expect("a float");
            assert_eq!(must_marshal(&value), text);
        }
    }
}
