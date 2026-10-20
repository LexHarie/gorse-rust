// Ports: spf13/viper v1.21.0 viper.go (SetDefault, BindEnv, getEnv, ReadInConfig, getConfigType, unmarshalReader, Get, find, searchIndexableWithPathPrefixes, searchSliceWithPathPrefixes, searchMapWithPathPrefixes, searchMap, isPathShadowedInDeepMap, AllKeys, flattenAndMergeMap, mergeFlatMap, getSettings, UnsupportedConfigError, ConfigParseError) and util.go (insensitiviseMap, insensitiviseVal, insensitiveArray, deepSearch) @ add21b5
//
// Derived from spf13/viper: Copyright (c) 2014 Steve Francia. Use of this
// source code is governed by the MIT license that can be found in
// crates/gorse-config/licenses/VIPER-LICENSE.

//! The part of viper `config.LoadConfig` uses: defaults, environment
//! bindings, one configuration file, and the merged settings it hands to
//! mapstructure.
//!
//! Go's viper builds the merged settings by iterating Go maps, so a few inputs
//! give different results from run to run. This port iterates in a fixed
//! order, which always gives one of Go's results (`PORTING_NOTES.md`,
//! "Configuration"):
//!
//! - the keys of the merged settings are visited in sorted order, so a key is
//!   set before the keys under it;
//! - a file mapping is lower-cased in document order, so of two spellings of a
//!   key that are not lower-case the later one wins;
//! - mapping keys that `cast.ToString` makes equal keep the later value.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};

use gorse_common::gostrconv;
use gorse_common::gotime::errno_text;
use gorse_common::gounicode::strings_to_lower;

use crate::formats;
use crate::value::{Map, Value, cast_to_string_map};

/// viper's `SupportedExts`.
pub const SUPPORTED_EXTS: &[&str] = &[
    "json",
    "toml",
    "yaml",
    "yml",
    "properties",
    "props",
    "prop",
    "hcl",
    "tfvars",
    "dotenv",
    "env",
    "ini",
];

/// The key delimiter.
const DELIM: &str = ".";

/// An error from [`Viper::read_in_config`], with Go's text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// `UnsupportedConfigError`: the file's extension is not one viper knows.
    #[error("Unsupported Config Type {}", gostrconv::quote(.0))]
    Unsupported(String),
    /// An `*os.PathError` opening or reading the file.
    #[error("{0}")]
    Io(String),
    /// `ConfigParseError`: no decoder for the format, or the decoder's error.
    #[error("While parsing config: {0}")]
    Parse(String),
}

/// `*os.PathError.Error()`: the operation, the path and the errno's text. An
/// error std raises without a system call (a path with a NUL byte) is the
/// `EINVAL` Go's syscall package gives it.
pub(crate) fn path_error(op: &str, path: &str, err: &io::Error) -> String {
    const EINVAL: i32 = 22;
    format!(
        "{op} {path}: {}",
        errno_text(err.raw_os_error().unwrap_or(EINVAL))
    )
}

/// A viper instance with the layers `LoadConfig` fills: defaults, environment
/// bindings and the configuration file.
pub struct Viper<'e> {
    defaults: Map,
    env: Vec<(String, Vec<String>)>,
    config: Map,
    lookup_env: &'e dyn Fn(&str) -> Option<String>,
}

impl<'e> Viper<'e> {
    /// `viper.New()` (or `viper.Reset()`), reading environment variables with
    /// `lookup_env` (Go's `os.LookupEnv`).
    pub fn new(lookup_env: &'e dyn Fn(&str) -> Option<String>) -> Self {
        Viper {
            defaults: Map::new(),
            env: Vec::new(),
            config: Map::new(),
            lookup_env,
        }
    }

    /// `SetDefault`. The defaults Gorse sets are not maps, which viper would
    /// also lower-case.
    pub fn set_default(&mut self, key: &str, value: Value) {
        let key = strings_to_lower(key);
        let value = insensitivise_val(value);
        let path: Vec<&str> = key.split(DELIM).collect();
        let (last, parents) = path.split_last().expect("split yields at least one element");
        deep_search(&mut self.defaults, parents).insert(strings_to_lower(last), value);
    }

    /// `BindEnv(key, env)`.
    pub fn bind_env(&mut self, key: &str, env: &str) {
        let key = strings_to_lower(key);
        match self.env.iter_mut().find(|(k, _)| *k == key) {
            Some((_, envs)) => envs.push(env.to_owned()),
            None => self.env.push((key, vec![env.to_owned()])),
        }
    }

    /// `getEnv`: a variable that is set but empty counts as unset.
    fn get_env(&self, name: &str) -> Option<String> {
        (self.lookup_env)(name).filter(|v| !v.is_empty())
    }

    /// `SetConfigFile(path)` and `ReadInConfig()`: picks the decoder by the
    /// file's extension, reads the file and lower-cases its keys.
    pub fn read_in_config(&mut self, path: &str) -> Result<(), ReadError> {
        let config_type = config_type(path);
        if !SUPPORTED_EXTS.contains(&config_type) {
            return Err(ReadError::Unsupported(config_type.to_owned()));
        }
        let bytes = read_file(path).map_err(ReadError::Io)?;
        let mut config = self.unmarshal_reader(config_type, &bytes)?;
        insensitivise_map(&mut config);
        self.config = config;
        Ok(())
    }

    /// `unmarshalReader` without `insensitiviseMap`: the codec for the format
    /// (viper's `DefaultCodecRegistry`).
    fn unmarshal_reader(&self, config_type: &str, bytes: &[u8]) -> Result<Map, ReadError> {
        // The type passed the case-sensitive `SupportedExts` check, so it is
        // already lower-case.
        let decoded = match strings_to_lower(config_type).as_str() {
            "json" => formats::json::decode(bytes),
            "toml" => formats::toml::decode(bytes),
            "yaml" | "yml" => formats::yaml::decode(bytes),
            "dotenv" | "env" => formats::dotenv::decode(bytes, self.lookup_env),
            _ => Err("decoder not found for this format".to_owned()),
        };
        decoded.map_err(ReadError::Parse)
    }

    /// `Get(key)`: the value from the first layer that has it, or
    /// [`Value::Nil`].
    pub fn get(&self, key: &str) -> Value {
        self.find(&strings_to_lower(key))
    }

    /// `find`, for the layers `LoadConfig` fills (no aliases, overrides, flags
    /// or key/value store). A value under a path that a layer holds as
    /// something other than a map is shadowed: the lookup stops there.
    fn find(&self, lcase_key: &str) -> Value {
        let path: Vec<&str> = lcase_key.split(DELIM).collect();
        let nested = path.len() > 1;

        if let Some((_, envs)) = self.env.iter().find(|(k, _)| k == lcase_key) {
            for env in envs {
                if let Some(v) = self.get_env(env) {
                    return Value::String(v);
                }
            }
        }
        // `isPathShadowedInFlatMap(path, v.env)` is always "": v.env is a
        // `map[string][]string`, which the function does not handle.

        if let Some(v) = search_indexable_with_path_prefixes(&Value::Map(self.config.clone()), &path) {
            return v;
        }
        if nested && is_path_shadowed_in_deep_map(&path, &self.config) {
            return Value::Nil;
        }

        if let Some(v) = non_nil(search_map(&self.defaults, &path)) {
            return v;
        }
        Value::Nil
    }

    /// `AllKeys`, sorted (Go returns them in random order).
    pub fn all_keys(&self) -> Vec<String> {
        let mut shadow = BTreeSet::new();
        // mergeFlatMap(v.env): no bound key is under another one, so the
        // parent check never skips one and the order does not matter.
        'outer: for (k, _) in &self.env {
            let path: Vec<&str> = k.split(DELIM).collect();
            for i in 1..path.len() {
                if shadow.contains(&path[..i].join(DELIM)) {
                    continue 'outer;
                }
            }
            shadow.insert(strings_to_lower(k));
        }
        flatten_and_merge_map(&mut shadow, &self.config, "");
        flatten_and_merge_map(&mut shadow, &self.defaults, "");
        shadow.into_iter().collect()
    }

    /// `AllSettings` (`getSettings(AllKeys())`): every key's value in a nested
    /// map. Keys are visited in sorted order; Go visits them in random order,
    /// which matters only when a key and a key under it both have values.
    pub fn all_settings(&self) -> Map {
        let mut m = Map::new();
        for k in self.all_keys() {
            let value = self.get(&k);
            if matches!(value, Value::Nil) {
                continue;
            }
            let path: Vec<&str> = k.split(DELIM).collect();
            let (last, parents) = path.split_last().expect("split yields at least one element");
            deep_search(&mut m, parents).insert(strings_to_lower(last), value);
        }
        m
    }
}

/// `getConfigType`: the file's extension without the dot (`filepath.Ext`),
/// or "" if it has none or only the dot.
fn config_type(path: &str) -> &str {
    let ext = path
        .rfind(['.', '/'])
        .filter(|&i| path.as_bytes()[i] == b'.')
        .map_or("", |i| &path[i..]);
    if ext.len() > 1 { &ext[1..] } else { "" }
}

/// `afero.ReadFile` on the OS file system: `os.Open`, then reading to the end.
fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| path_error("open", path, &e))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| path_error("read", path, &e))?;
    Ok(bytes)
}

/// A Go `any` that is not nil.
fn non_nil(v: Option<Value>) -> Option<Value> {
    v.filter(|v| !matches!(v, Value::Nil))
}

/// `insensitiviseMap`, in document order: each key's current value is
/// lower-cased in turn and stored under the lower-cased key, and a key that
/// was not lower-case is removed. A lower-case key thus loses to any other
/// spelling of it, and of two other spellings the later one wins.
fn insensitivise_map(m: &mut Map) {
    let keys: Vec<String> = m.keys().map(str::to_owned).collect();
    for key in keys {
        // Every key is still there: a key is only removed in its own turn.
        let Some(val) = m.get(&key).cloned() else {
            continue;
        };
        let val = insensitivise_val(val);
        let lower = strings_to_lower(&key);
        if key != lower {
            m.remove(&key);
        }
        m.insert(lower, val);
    }
}

/// `insensitiviseVal`: maps (after `cast.ToStringMap`) and the maps in slices
/// are lower-cased; other values are kept.
fn insensitivise_val(val: Value) -> Value {
    match val {
        Value::AnyMap(entries) => {
            let mut m = cast_to_string_map(&entries);
            insensitivise_map(&mut m);
            Value::Map(m)
        }
        Value::Map(mut m) => {
            insensitivise_map(&mut m);
            Value::Map(m)
        }
        Value::Slice(items) => Value::Slice(items.into_iter().map(insensitivise_val).collect()),
        other => other,
    }
}

/// `deepSearch`: the map at `path` under `m`, creating maps where keys are
/// missing and replacing values that are not `map[string]any`.
fn deep_search<'m>(mut m: &'m mut Map, path: &[&str]) -> &'m mut Map {
    for &k in path {
        if !matches!(m.get(k), Some(Value::Map(_))) {
            m.insert(k.to_owned(), Value::Map(Map::new()));
        }
        let Some(Value::Map(next)) = m.get_mut(k) else {
            unreachable!("the key was just set to a map");
        };
        m = next;
    }
    m
}

/// `searchIndexableWithPathPrefixes`: the value at `path`, where a key in a
/// map may itself contain the delimiter. Longer prefixes of the path are
/// tried first.
fn search_indexable_with_path_prefixes(source: &Value, path: &[&str]) -> Option<Value> {
    if path.is_empty() {
        return non_nil(Some(source.clone()));
    }
    for i in (1..=path.len()).rev() {
        let prefix_key = strings_to_lower(&path[..i].join(DELIM));
        let val = match source {
            Value::Slice(items) => search_slice_with_path_prefixes(items, &prefix_key, i, path),
            Value::Map(m) => search_map_with_path_prefixes(m, &prefix_key, i, path),
            _ => None,
        };
        if let Some(v) = non_nil(val) {
            return Some(v);
        }
    }
    None
}

/// `searchSliceWithPathPrefixes`: the prefix is an index into the slice.
fn search_slice_with_path_prefixes(
    items: &[Value],
    prefix_key: &str,
    path_index: usize,
    path: &[&str],
) -> Option<Value> {
    let (index, err) = gostrconv::atoi(prefix_key);
    // A negative index would panic in Go. It cannot happen here: no key that
    // `LoadConfig` looks up has a number after a part that holds a slice.
    let index = usize::try_from(index).ok().filter(|_| err.is_none())?;
    let next = items.get(index)?;
    if path_index == path.len() {
        return Some(next.clone());
    }
    search_next(next, &path[path_index..])
}

/// `searchMapWithPathPrefixes`: the prefix is a key of the map.
fn search_map_with_path_prefixes(
    m: &Map,
    prefix_key: &str,
    path_index: usize,
    path: &[&str],
) -> Option<Value> {
    let next = m.get(prefix_key)?;
    if path_index == path.len() {
        return Some(next.clone());
    }
    search_next(next, &path[path_index..])
}

/// The rest of the path under a value found by a prefix: maps and slices are
/// searched, anything else ends the search.
fn search_next(next: &Value, rest: &[&str]) -> Option<Value> {
    match next {
        Value::AnyMap(entries) => {
            search_indexable_with_path_prefixes(&Value::Map(cast_to_string_map(entries)), rest)
        }
        Value::Map(_) | Value::Slice(_) => search_indexable_with_path_prefixes(next, rest),
        _ => None,
    }
}

/// `searchMap`: the value at `path`, one key per element.
fn search_map(source: &Map, path: &[&str]) -> Option<Value> {
    let (first, rest) = path.split_first()?;
    let next = source.get(first)?;
    if rest.is_empty() {
        return Some(next.clone());
    }
    match next {
        Value::AnyMap(entries) => search_map(&cast_to_string_map(entries), rest),
        Value::Map(m) => search_map(m, rest),
        _ => None,
    }
}

/// `isPathShadowedInDeepMap`: whether a proper prefix of `path` holds a
/// value that is not a map.
fn is_path_shadowed_in_deep_map(path: &[&str], m: &Map) -> bool {
    for i in 1..path.len() {
        match non_nil(search_map(m, &path[..i])) {
            None => return false,
            Some(Value::Map(_) | Value::AnyMap(_)) => continue,
            Some(_) => return true,
        }
    }
    false
}

/// `flattenAndMergeMap`: the paths of the leaves of `m` (values that are not
/// maps, empty maps giving none), except under a path already a key.
fn flatten_and_merge_map(shadow: &mut BTreeSet<String>, m: &Map, prefix: &str) {
    if !prefix.is_empty() && shadow.contains(prefix) {
        return;
    }
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}{DELIM}")
    };
    for (k, val) in m.iter() {
        let full_key = format!("{prefix}{k}");
        match val {
            Value::Map(m2) => flatten_and_merge_map(shadow, m2, &full_key),
            Value::AnyMap(entries) => {
                flatten_and_merge_map(shadow, &cast_to_string_map(entries), &full_key)
            }
            _ => {
                shadow.insert(strings_to_lower(&full_key));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn map(entries: &[(&str, Value)]) -> Map {
        Map::from(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn config_type_is_the_extension() {
        assert_eq!(config_type("/a/config.toml"), "toml");
        assert_eq!(config_type("/a/config.TOML"), "TOML");
        assert_eq!(config_type("/a/config"), "");
        assert_eq!(config_type("/a.d/config"), "");
        assert_eq!(config_type("/a/config."), "");
        assert_eq!(config_type("/a/.env"), "env");
        assert_eq!(config_type("x.tar.gz"), "gz");
    }

    #[test]
    fn insensitivise_keeps_the_last_other_spelling() {
        let mut m = map(&[
            ("Port", Value::Int64(1)),
            ("PORT", Value::Int64(2)),
            ("port", Value::Int64(3)),
            ("Host", Value::Map(map(&[("A", Value::Int64(4))]))),
        ]);
        insensitivise_map(&mut m);
        assert_eq!(m.get("port"), Some(&Value::Int64(2)));
        assert_eq!(m.get("Port"), None);
        assert_eq!(
            m.get("host"),
            Some(&Value::Map(map(&[("a", Value::Int64(4))])))
        );
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn find_walks_env_config_and_defaults() {
        let env = |name: &str| match name {
            "SET" => Some("from env".to_owned()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        let mut v = Viper::new(&env);
        v.set_default("a.b", Value::Int(1));
        v.set_default("a.c", Value::Int(2));
        v.set_default("d.e", Value::Int(3));
        v.bind_env("a.b", "SET");
        v.bind_env("a.c", "EMPTY");
        v.config = map(&[("d", Value::Int64(9)), ("x.y", Value::Int64(7))]);
        assert_eq!(v.get("A.B"), Value::String("from env".into()));
        assert_eq!(v.get("a.c"), Value::Int(2));
        // `d` is not a map in the file, so the default under it is shadowed.
        assert_eq!(v.get("d.e"), Value::Nil);
        // A key with the delimiter in it.
        assert_eq!(v.get("x.y"), Value::Int64(7));
        assert_eq!(v.all_keys(), ["a.b", "a.c", "d", "d.e", "x.y"]);
    }

    #[test]
    fn all_settings_nests_keys() {
        let mut v = Viper::new(&no_env);
        v.set_default("a.b", Value::Int(1));
        v.config = map(&[("a.c", Value::Int64(2))]);
        let settings = v.all_settings();
        assert_eq!(
            settings,
            map(&[(
                "a",
                Value::Map(map(&[("b", Value::Int(1)), ("c", Value::Int64(2))]))
            )])
        );
    }
}
