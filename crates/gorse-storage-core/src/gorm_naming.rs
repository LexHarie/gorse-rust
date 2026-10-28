// Ports: gorm.io/gorm schema/naming.go (NamingStrategy) @ v1.31.1 (the version add21b5's go.mod pins)
//
// Derived from GORM: Copyright (c) 2013-present Jinzhu; MIT license in
// crates/gorse-storage-core/licenses/GORM-LICENSE.

//! gorm's `schema.NamingStrategy`: how gorm turns Go type and field names into
//! table, column, index and constraint names.
//!
//! The Go SQL stores hand Gorse's models to gorm (`AutoMigrate`, `Create`,
//! `Find`) under `storage.NewGORMConfig`, so the names in the databases a Go
//! node creates come from this code. The Rust stores write their SQL by hand
//! and use this to get the same names for any table prefix.
//!
//! Go strings are bytes and gorm mixes byte and rune indexing, so names here
//! are bytes and the output can be invalid UTF-8 exactly where gorm's is.
//!
//! Only what Gorse reaches is ported. `NewGORMConfig` always sets
//! `SingularTable: true`, so the plural forms (`inflection.Plural`, the
//! `jinzhu/inflection` package) are left out and there is no `SingularTable`
//! field: every name is singular. `SchemaName` (table name back to a Go type
//! name) and `RelationshipFKName` (needs `schema.Relationship`; Gorse has no
//! relationships) are not ported.

use std::sync::LazyLock;

use gorse_common::gostrings;
use sha1::{Digest, Sha1};

/// gorm's `schema.Replacer`: anything with `Replace(name string) string`,
/// such as a `*strings.Replacer`.
pub trait Replacer {
    /// Go's `Replace`.
    fn replace(&self, name: &[u8]) -> Vec<u8>;
}

impl Replacer for gostrings::Replacer {
    fn replace(&self, name: &[u8]) -> Vec<u8> {
        gostrings::Replacer::replace(self, name)
    }
}

impl<F: Fn(&[u8]) -> Vec<u8>> Replacer for F {
    fn replace(&self, name: &[u8]) -> Vec<u8> {
        self(name)
    }
}

/// gorm's `schema.NamingStrategy` with `SingularTable: true` (see the module
/// docs).
#[derive(Default)]
pub struct NamingStrategy {
    /// Go's `TablePrefix`.
    pub table_prefix: Vec<u8>,
    /// Go's `NameReplacer` (nil is `None`).
    pub name_replacer: Option<Box<dyn Replacer + Send + Sync>>,
    /// Go's `NoLowerCase`.
    pub no_lower_case: bool,
    /// Go's `IdentifierMaxLength`; 0 means 64.
    pub identifier_max_length: i64,
}

impl std::fmt::Debug for NamingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamingStrategy")
            .field("table_prefix", &String::from_utf8_lossy(&self.table_prefix))
            .field("name_replacer", &self.name_replacer.as_ref().map(|_| ".."))
            .field("no_lower_case", &self.no_lower_case)
            .field("identifier_max_length", &self.identifier_max_length)
            .finish()
    }
}

impl NamingStrategy {
    /// Go's `TableName`: the prefix and the name in snake case (singular).
    pub fn table_name(&self, s: impl AsRef<[u8]>) -> Vec<u8> {
        let mut out = self.table_prefix.clone();
        out.extend_from_slice(&self.to_db_name(s.as_ref()));
        out
    }

    /// Go's `ColumnName`: the column in snake case; the table is unused.
    pub fn column_name(&self, _table: impl AsRef<[u8]>, column: impl AsRef<[u8]>) -> Vec<u8> {
        self.to_db_name(column.as_ref())
    }

    /// Go's `JoinTableName` (singular): a name that is already lower case
    /// (and `no_lower_case` is off) only gets the prefix; any other goes
    /// through the replacer and snake case.
    pub fn join_table_name(&self, s: impl AsRef<[u8]>) -> Vec<u8> {
        let s = s.as_ref();
        if !self.no_lower_case && is_lower(s) {
            let mut out = self.table_prefix.clone();
            out.extend_from_slice(s);
            return out;
        }
        self.table_name(s)
    }

    /// Go's `CheckerName`: `chk_<table>_<column>`, the column as given.
    pub fn checker_name(&self, table: impl AsRef<[u8]>, column: impl AsRef<[u8]>) -> Vec<u8> {
        self.format_name(b"chk", table.as_ref(), column.as_ref())
    }

    /// Go's `IndexName`: `idx_<table>_<column in snake case>`.
    pub fn index_name(&self, table: impl AsRef<[u8]>, column: impl AsRef<[u8]>) -> Vec<u8> {
        self.format_name(b"idx", table.as_ref(), &self.to_db_name(column.as_ref()))
    }

    /// Go's `UniqueName`: `uni_<table>_<column in snake case>`.
    pub fn unique_name(&self, table: impl AsRef<[u8]>, column: impl AsRef<[u8]>) -> Vec<u8> {
        self.format_name(b"uni", table.as_ref(), &self.to_db_name(column.as_ref()))
    }

    /// Go's `formatName`: `prefix_table_name` with every `.` made `_`; a name
    /// of more runes than the identifier limit is cut to limit-8 bytes (which
    /// can split a rune, as in Go) and ends in 8 hex digits of its SHA-1.
    fn format_name(&self, prefix: &[u8], table: &[u8], name: &[u8]) -> Vec<u8> {
        let mut joined = Vec::with_capacity(prefix.len() + table.len() + name.len() + 2);
        joined.extend_from_slice(prefix);
        joined.push(b'_');
        joined.extend_from_slice(table);
        joined.push(b'_');
        joined.extend_from_slice(name);
        let mut formatted_name = gostrings::replace_all(&joined, b".", b"_");

        let identifier_max_length = if self.identifier_max_length == 0 {
            64
        } else {
            self.identifier_max_length
        };

        if rune_count(&formatted_name) as i64 > identifier_max_length {
            let bs = Sha1::digest(&formatted_name);
            let Ok(cut) = usize::try_from(identifier_max_length - 8) else {
                // Go slices formattedName[0:IdentifierMaxLength-8] and panics
                // on a negative bound.
                panic!(
                    "runtime error: slice bounds out of range [:{}]",
                    identifier_max_length - 8
                );
            };
            formatted_name.truncate(cut);
            for b in &bs[..4] {
                formatted_name.extend_from_slice(format!("{b:02x}").as_bytes());
            }
        }
        formatted_name
    }

    /// Go's `toDBName`: the replacer, then (unless `no_lower_case`) the
    /// common initialisms title-cased and the name turned into snake case.
    fn to_db_name(&self, name: &[u8]) -> Vec<u8> {
        if name.is_empty() {
            return Vec::new();
        }

        let mut name = name.to_vec();
        if let Some(replacer) = &self.name_replacer {
            let tmp_name = replacer.replace(&name);

            if tmp_name.is_empty() {
                return name;
            }

            name = tmp_name;
        }

        if self.no_lower_case {
            return name;
        }

        let value = COMMON_INITIALISMS_REPLACER.replace(&name);
        let mut buf = Vec::with_capacity(value.len() + 4);
        let is_upper = |b: u8| b.is_ascii_uppercase();
        let is_digit = |b: u8| b.is_ascii_digit();
        let mut last_case = false; // upper case == true
        let mut cur_case = is_upper(value[0]);

        // Go ranges over the runes of value[:len(value)-1] while it reads
        // the neighbouring bytes by index.
        let head = &value[..value.len() - 1];
        let mut i = 0;
        while i < head.len() {
            let (v, width) = decode_rune(&head[i..]);
            let next_case = is_upper(value[i + 1]);
            let next_number = is_digit(value[i + 1]);

            if cur_case {
                if last_case && (next_case || next_number) {
                    write_rune(&mut buf, v + 32);
                } else {
                    if i > 0 && value[i - 1] != b'_' && value[i + 1] != b'_' {
                        buf.push(b'_');
                    }
                    write_rune(&mut buf, v + 32);
                }
            } else {
                write_rune(&mut buf, v);
            }

            last_case = cur_case;
            cur_case = next_case;
            i += width;
        }

        let last = value[value.len() - 1];
        if cur_case {
            if !last_case && value.len() > 1 {
                buf.push(b'_');
            }
            buf.push(last.wrapping_add(32));
        } else {
            buf.push(last);
        }
        buf
    }
}

/// gorm's `commonInitialisms`
/// (<https://github.com/golang/lint/blob/master/lint.go#L770>).
pub const COMMON_INITIALISMS: &[&str] = &[
    "API", "ASCII", "CPU", "CSS", "DNS", "EOF", "GUID", "HTML", "HTTP", "HTTPS", "ID", "IP",
    "JSON", "LHS", "QPS", "RAM", "RHS", "RPC", "SLA", "SMTP", "SSH", "TLS", "TTL", "UID", "UI",
    "UUID", "URI", "URL", "UTF8", "VM", "XML", "XSRF", "XSS",
];

/// `cases.Title(language.Und).String(initialism)`. Every initialism is one
/// ASCII word, which Title writes with its first letter upper case and the
/// rest lower case ("UTF8" becomes "Utf8").
fn title(initialism: &str) -> String {
    let lower = initialism.to_ascii_lowercase();
    let mut out = lower[..1].to_ascii_uppercase();
    out.push_str(&lower[1..]);
    out
}

/// gorm's `commonInitialismsReplacer`: each initialism to its title case, in
/// list order (so "HTTP" wins over "HTTPS" and "UID" over "UI").
static COMMON_INITIALISMS_REPLACER: LazyLock<gostrings::Replacer> = LazyLock::new(|| {
    let pairs: Vec<(String, String)> = COMMON_INITIALISMS
        .iter()
        .map(|&i| (i.to_owned(), title(i)))
        .collect();
    gostrings::Replacer::new(&pairs)
});

/// Go's `utf8.DecodeRuneInString`: the rune at the start of s and its width;
/// an invalid or truncated sequence is U+FFFD of width 1.
fn decode_rune(s: &[u8]) -> (u32, usize) {
    match s
        .utf8_chunks()
        .next()
        .and_then(|c| c.valid().chars().next())
    {
        Some(c) => (u32::from(c), c.len_utf8()),
        None => (0xFFFD, 1),
    }
}

/// Go's `strings.Builder.WriteRune`: the UTF-8 encoding of r, U+FFFD for a
/// value that is not a valid rune.
fn write_rune(buf: &mut Vec<u8>, r: u32) {
    let c = char::from_u32(r).unwrap_or('\u{FFFD}');
    let mut tmp = [0; 4];
    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
}

/// Go's `utf8.RuneCountInString`: each invalid byte counts as one rune.
fn rune_count(s: &[u8]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < s.len() {
        i += decode_rune(&s[i..]).1;
        n += 1;
    }
    n
}

/// `strings.ToLower(s) == s`. Go's `ToLower` writes U+FFFD for an invalid
/// byte, so invalid UTF-8 is never equal; otherwise every rune must be its own
/// lower case. Rust's `char::to_lowercase` agrees with Go's simple mapping on
/// whether a rune changes (it only differs on what U+0130 becomes) for the
/// code points both Unicode versions assign.
fn is_lower(s: &[u8]) -> bool {
    if s.is_ascii() {
        return !s.iter().any(u8::is_ascii_uppercase);
    }
    let Ok(s) = std::str::from_utf8(s) else {
        return false;
    };
    s.chars().all(|c| {
        let mut lower = c.to_lowercase();
        lower.next() == Some(c) && lower.next().is_none()
    })
}

#[doc(hidden)]
pub mod internal {
    //! Unexported gorm helpers that gorm's own tests call; the Rust ports of
    //! those tests use them from here. Not part of the API.

    use super::NamingStrategy;

    /// gorm's `formatName`.
    pub fn format_name(ns: &NamingStrategy, prefix: &[u8], table: &[u8], name: &[u8]) -> Vec<u8> {
        ns.format_name(prefix, table, name)
    }

    /// gorm's `toDBName`.
    pub fn to_db_name(ns: &NamingStrategy, name: &[u8]) -> Vec<u8> {
        ns.to_db_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The title-cased initialisms as golang.org/x/text/cases gives them
    // (`cases.Title(language.Und).String`, run on go1.27.0 with the
    // golang.org/x/text version add21b5's go.sum pins).
    #[test]
    fn initialisms_title_case_like_go() {
        let want = [
            "Api", "Ascii", "Cpu", "Css", "Dns", "Eof", "Guid", "Html", "Http", "Https", "Id",
            "Ip", "Json", "Lhs", "Qps", "Ram", "Rhs", "Rpc", "Sla", "Smtp", "Ssh", "Tls", "Ttl",
            "Uid", "Ui", "Uuid", "Uri", "Url", "Utf8", "Vm", "Xml", "Xsrf", "Xss",
        ];
        let got: Vec<String> = COMMON_INITIALISMS.iter().map(|i| title(i)).collect();
        assert_eq!(got, want);
    }

    // Values printed by gorm v1.31.1 on go1.27.0 for NamingStrategy{}: the
    // byte and rune mixing of toDBName and formatName.
    #[test]
    fn to_db_name_byte_quirks_like_go() {
        let ns = NamingStrategy::default();
        let cases: &[(&[u8], &[u8])] = &[
            ("aÄ".as_bytes(), b"a\xef\xbf\xbd\x84"),
            ("ÄB".as_bytes(), "ÄB".as_bytes()),
            (b"A\xffB", b"a\xef\xbf\xbd_b"),
            (b"\xffA", b"\xef\xbf\xbd_a"),
            (b"HTTPSServer", b"http_s_server"),
            (b"UIDx", b"uidx"),
            (b"a_B", b"a__b"),
            (b"_A", b"__a"),
            (b"A_", b"a_"),
            (b"AB_CD", b"ab_cd"),
            (b"X9Y", b"x9_y"),
            ("ÉcoleID".as_bytes(), "École_id".as_bytes()),
            (b"\xe2\x82AB", b"\xef\xbf\xbd\xef\xbf\xbd_ab"),
            (b"aB\xe2\x82", b"a_b\xef\xbf\xbd\x82"),
            (b"A", b"a"),
            (b"AA", b"aa"),
            (b"aA", b"a_a"),
            (b"Aa", b"aa"),
            (b"UserID", b"user_id"),
            (b"IDs", b"ids"),
            ("ÄÖ".as_bytes(), b"\xc3\x84\xef\xbf\xbd\x96"),
            ("AÄ".as_bytes(), b"a\xef\xbf\xbd\x84"),
        ];
        for &(input, want) in cases {
            assert_eq!(
                ns.column_name("", input),
                want,
                "ColumnName({:?})",
                String::from_utf8_lossy(input)
            );
        }

        let long = format!("Ä{}", "a".repeat(90));
        assert_eq!(
            ns.index_name("t", long),
            "idx_t_Äaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaafe3954a4".as_bytes()
        );
        assert_eq!(
            ns.index_name("t".repeat(53), "ÄÄÄÄÄÄÄÄ"),
            b"idx_tttttttttttttttttttttttttttttttttttttttttttttttttttt9ce0c424"
        );
        assert_eq!(ns.checker_name("a.b", "c.d"), b"chk_a_b_c_d");
        assert_eq!(ns.join_table_name("ünïcode"), "ünïcode".as_bytes());
        // Go's ToLower writes U+FFFD for the invalid byte, so the name is not
        // "lower case" and goes through toDBName (which keeps it).
        assert_eq!(ns.join_table_name(b"bad\xff"), b"bad\xff");
    }

    #[test]
    #[should_panic(expected = "slice bounds out of range [:-3]")]
    fn format_name_panics_like_go_below_eight() {
        let ns = NamingStrategy {
            identifier_max_length: 5,
            ..NamingStrategy::default()
        };
        let _ = ns.index_name("table", "column");
    }
}
