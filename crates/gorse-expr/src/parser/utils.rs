// Ports: github.com/expr-lang/expr/parser/utils/utils.go @ v1.17.8

//! Character classes of the lexer, from Go's `unicode` tables (see
//! `contracts/tools/exprunicode`), so they follow Go's Unicode version
//! rather than Rust's.

use crate::gostr::in_table;
use crate::unicode_tables::{DIGIT, LETTER, SPACE};

/// `IsValidIdentifier`: a letter, `_` or `$`, then letters, digits, `_` or
/// `$`.
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(h) if is_alphabetic(h) => chars.all(is_alpha_numeric),
        _ => false,
    }
}

/// `IsSpace`: Go's `unicode.IsSpace`.
pub fn is_space(r: char) -> bool {
    in_table(SPACE, r)
}

/// `IsAlphaNumeric`: [`is_alphabetic`] or a decimal digit (`unicode.IsDigit`).
pub fn is_alpha_numeric(r: char) -> bool {
    is_alphabetic(r) || in_table(DIGIT, r)
}

/// `IsAlphabetic`: `_`, `$` or a letter (`unicode.IsLetter`).
pub fn is_alphabetic(r: char) -> bool {
    r == '_' || r == '$' || in_table(LETTER, r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gostr::is_print;

    /// The code points Go's class accepts, from the golden file.
    fn go_class(golden: &serde_json::Value, name: &str) -> Vec<bool> {
        let mut set = vec![false; 0x11_0000];
        for range in golden["unicode"][name].as_array().expect("ranges") {
            let lo = range[0].as_u64().expect("lo");
            let hi = range[1].as_u64().expect("hi");
            for c in lo..=hi {
                set[usize::try_from(c).expect("code point")] = true;
            }
        }
        set
    }

    /// unicode.IsLetter, unicode.IsDigit, unicode.IsSpace and
    /// strconv.IsPrint, for every code point.
    #[test]
    fn character_classes_match_go() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/parser_golden.json");
        let text = std::fs::read_to_string(path).expect("golden");
        let golden: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        type Class = fn(char) -> bool;
        let classes: [(&str, Class); 4] = [
            ("letter", |c| is_alphabetic(c) && c != '_' && c != '$'),
            ("digit", |c| is_alpha_numeric(c) && !is_alphabetic(c)),
            ("space", is_space),
            ("print", is_print),
        ];
        for (name, class) in classes {
            let go = go_class(&golden, name);
            let mut count = 0;
            for (code, &want) in go.iter().enumerate() {
                let code = u32::try_from(code).expect("code point");
                // Go's classes reject surrogates, which are not chars.
                let got = char::from_u32(code).is_some_and(class);
                assert_eq!(got, want, "{name} U+{code:04X}");
                count += usize::from(want);
            }
            assert!(count > 0, "{name} is empty");
        }
        assert!(is_valid_identifier("_a$1"));
        assert!(!is_valid_identifier("1a"));
        assert!(!is_valid_identifier(""));
    }
}
