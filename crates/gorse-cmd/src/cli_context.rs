// Ports: cmd/gorse-cli/context.go @ add21b5 (the contexts, kept in the system keyring) and the endpoint and key lookup of cmd/gorse-cli/main.go

//! The CLI's contexts: a named endpoint and API key, kept in the system keyring
//! under the keys Go's `gorse-cli` uses, so the entries are the same names.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The keyring service that holds the contexts (Go's `keyringService`).
pub const KEYRING_SERVICE: &str = "gorse-cli";

const CURRENT_CONTEXT_KEY: &str = "current-context";
const CONTEXTS_KEY: &str = "contexts";

/// The context name pattern (Go's `contextNamePattern`), as it is written.
pub const CONTEXT_NAME_PATTERN: &str = "^[A-Za-z0-9_.-]+$";

/// A store of secrets by service and key, as Go's `go-keyring` is.
pub trait Keyring: Send + Sync {
    /// The value of `key`, or `None` when there is none.
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, String>;
    /// Stores `value` under `key`, replacing the last value.
    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), String>;
    /// Removes `key`. A key that is not there is not an error.
    fn delete(&self, service: &str, key: &str) -> Result<(), String>;
}

/// The operating system's keyring, through the `keyring` crate.
#[derive(Debug, Default)]
pub struct SystemKeyring;

impl Keyring for SystemKeyring {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(service, key).map_err(|err| err.to_string())?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(service, key).map_err(|err| err.to_string())?;
        entry.set_password(value).map_err(|err| err.to_string())
    }

    fn delete(&self, service: &str, key: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(service, key).map_err(|err| err.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }
}

/// An in-process keyring, for tests and for runs that must not touch the system's.
#[derive(Debug, Default)]
pub struct MemoryKeyring {
    entries: Mutex<BTreeMap<(String, String), String>>,
}

impl Keyring for MemoryKeyring {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, String> {
        let entries = self.entries.lock().map_err(|err| err.to_string())?;
        Ok(entries.get(&(service.to_owned(), key.to_owned())).cloned())
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), String> {
        let mut entries = self.entries.lock().map_err(|err| err.to_string())?;
        entries.insert((service.to_owned(), key.to_owned()), value.to_owned());
        Ok(())
    }

    fn delete(&self, service: &str, key: &str) -> Result<(), String> {
        let mut entries = self.entries.lock().map_err(|err| err.to_string())?;
        entries.remove(&(service.to_owned(), key.to_owned()));
        Ok(())
    }
}

/// A saved context: its name, endpoint and API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliContext {
    /// The context name.
    pub name: String,
    /// The master's base URL.
    pub endpoint: String,
    /// The admin API key.
    pub api_key: String,
}

/// The contexts of one keyring service (Go's `Contexts`).
pub struct Contexts<'a> {
    keyring: &'a dyn Keyring,
    service: &'a str,
}

/// Checks a context name against [`CONTEXT_NAME_PATTERN`].
pub fn valid_context_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

fn name_error(name: &str) -> String {
    format!(
        "context name {} must match {:?}",
        go_quote(name),
        CONTEXT_NAME_PATTERN
    )
}

/// Go's `strconv.Quote` for the names and values the CLI prints, which have no
/// control characters.
pub fn go_quote(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

impl<'a> Contexts<'a> {
    /// The contexts kept in `keyring` under `service`.
    pub fn new(keyring: &'a dyn Keyring, service: &'a str) -> Self {
        Self { keyring, service }
    }

    fn endpoint_key(name: &str) -> String {
        format!("context:{name}:admin-endpoint")
    }

    fn api_key_key(name: &str) -> String {
        format!("context:{name}:admin-api-key")
    }

    /// The names of the saved contexts, unique and sorted.
    pub fn names(&self) -> Result<Vec<String>, String> {
        let Some(raw) = self.keyring.get(self.service, CONTEXTS_KEY)? else {
            return Ok(Vec::new());
        };
        let names: Vec<String> = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
        Ok(unique_sorted(names))
    }

    fn save_names(&self, names: Vec<String>) -> Result<(), String> {
        let raw = serde_json::to_string(&unique_sorted(names)).map_err(|err| err.to_string())?;
        self.keyring.set(self.service, CONTEXTS_KEY, &raw)
    }

    fn add_name(&self, name: &str) -> Result<(), String> {
        let mut names = self.names()?;
        if !names.iter().any(|candidate| candidate == name) {
            names.push(name.to_owned());
        }
        self.save_names(names)
    }

    fn remove_name(&self, name: &str) -> Result<Vec<String>, String> {
        let names: Vec<String> = self
            .names()?
            .into_iter()
            .filter(|candidate| candidate != name)
            .collect();
        self.save_names(names.clone())?;
        Ok(names)
    }

    /// The name of the current context, or empty when there is none.
    pub fn current_name(&self) -> Result<String, String> {
        Ok(self
            .keyring
            .get(self.service, CURRENT_CONTEXT_KEY)?
            .unwrap_or_default())
    }

    /// Selects the context `name`.
    pub fn set_current_name(&self, name: &str) -> Result<(), String> {
        self.keyring.set(self.service, CURRENT_CONTEXT_KEY, name)
    }

    fn clear_current_name(&self) -> Result<(), String> {
        self.keyring.delete(self.service, CURRENT_CONTEXT_KEY)
    }

    /// Loads the context `name`. It fails when the name is invalid, or when the
    /// context is missing or incomplete.
    pub fn load(&self, name: &str) -> Result<CliContext, String> {
        if !valid_context_name(name) {
            return Err(name_error(name));
        }
        let endpoint = self.keyring.get(self.service, &Self::endpoint_key(name))?;
        let api_key = self.keyring.get(self.service, &Self::api_key_key(name))?;
        match (endpoint, api_key) {
            (Some(endpoint), Some(api_key)) => Ok(CliContext {
                name: name.to_owned(),
                endpoint,
                api_key,
            }),
            _ => Err(format!("context {} not found", go_quote(name))),
        }
    }

    /// Saves the context `name` and selects it. The endpoint and the API key must
    /// both be set.
    pub fn save(&self, name: &str, endpoint: &str, api_key: &str) -> Result<(), String> {
        if !valid_context_name(name) {
            return Err(name_error(name));
        }
        if endpoint.is_empty() {
            return Err("GORSE_ADMIN_ENDPOINT or --endpoint is required".to_owned());
        }
        if api_key.is_empty() {
            return Err("GORSE_ADMIN_API_KEY or --api-key is required".to_owned());
        }
        self.keyring
            .set(self.service, &Self::endpoint_key(name), endpoint)?;
        self.keyring
            .set(self.service, &Self::api_key_key(name), api_key)?;
        self.add_name(name)?;
        self.set_current_name(name)
    }

    /// Deletes the context `name`. When it was the current one, the first of the
    /// remaining contexts becomes current, or none does.
    pub fn delete(&self, name: &str) -> Result<(), String> {
        self.load(name)?;
        self.keyring
            .delete(self.service, &Self::endpoint_key(name))?;
        self.keyring
            .delete(self.service, &Self::api_key_key(name))?;
        let names = self.remove_name(name)?;
        if self.current_name()? != name {
            return Ok(());
        }
        match names.first() {
            None => self.clear_current_name(),
            Some(first) => self.set_current_name(first),
        }
    }
}

fn unique_sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

/// The endpoint and key a command should use, from its flags, the environment,
/// and the contexts, in that order. Empty strings are missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    /// The master's base URL, or empty.
    pub endpoint: String,
    /// The admin API key, or empty.
    pub api_key: String,
}

/// Go's `getEndpointAndKey` after the flags and the environment: the named
/// context from `--context`, and then the current context, fills what is still
/// missing. `Err` carries the message and suggestions of Go's `fatal`.
pub fn resolve(
    contexts: &Contexts<'_>,
    mut credentials: Credentials,
    context_flag: &str,
) -> Result<Credentials, (String, Vec<String>)> {
    if (credentials.endpoint.is_empty() || credentials.api_key.is_empty())
        && !context_flag.is_empty()
    {
        let context = contexts.load(context_flag).map_err(|_| {
            (
                format!("context {} was not found.", go_quote(context_flag)),
                vec![
                    "List available contexts:".to_owned(),
                    "  gorse-cli context list".to_owned(),
                    "Create it:".to_owned(),
                    format!(
                        "  gorse-cli context add {context_flag} --endpoint http://localhost:8088"
                    ),
                ],
            )
        })?;
        fill(&mut credentials, &context);
    }
    if credentials.endpoint.is_empty() || credentials.api_key.is_empty() {
        let current = contexts.current_name().map_err(|err| {
            (
                format!("failed to read the current context from the system keyring: {err}"),
                Vec::new(),
            )
        })?;
        if !current.is_empty() {
            let context = contexts.load(&current).map_err(|_| {
                (
                    format!(
                        "current context {} is invalid or incomplete.",
                        go_quote(&current)
                    ),
                    vec![
                        "Choose another context:".to_owned(),
                        "  gorse-cli context use <name>".to_owned(),
                        "Or recreate it:".to_owned(),
                        format!(
                            "  gorse-cli context add {current} --endpoint http://localhost:8088"
                        ),
                    ],
                )
            })?;
            fill(&mut credentials, &context);
        }
    }
    Ok(credentials)
}

fn fill(credentials: &mut Credentials, context: &CliContext) {
    if credentials.endpoint.is_empty() {
        credentials.endpoint = context.endpoint.clone();
    }
    if credentials.api_key.is_empty() {
        credentials.api_key = context.api_key.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_selects_and_lists_the_context() {
        let keyring = MemoryKeyring::default();
        let contexts = Contexts::new(&keyring, KEYRING_SERVICE);
        contexts
            .save("dev", "http://localhost:8088", "secret")
            .unwrap();
        assert_eq!(contexts.names().unwrap(), vec!["dev".to_owned()]);
        assert_eq!(contexts.current_name().unwrap(), "dev");
        let loaded = contexts.load("dev").unwrap();
        assert_eq!(loaded.endpoint, "http://localhost:8088");
        assert_eq!(loaded.api_key, "secret");
    }

    #[test]
    fn deleting_the_current_context_selects_the_first_remaining() {
        let keyring = MemoryKeyring::default();
        let contexts = Contexts::new(&keyring, KEYRING_SERVICE);
        contexts.save("b", "http://b", "k").unwrap();
        contexts.save("a", "http://a", "k").unwrap();
        contexts.delete("a").unwrap();
        assert_eq!(contexts.current_name().unwrap(), "b");
        contexts.delete("b").unwrap();
        assert_eq!(contexts.current_name().unwrap(), "");
        assert!(contexts.names().unwrap().is_empty());
    }

    #[test]
    fn invalid_names_are_rejected_with_go_text() {
        let keyring = MemoryKeyring::default();
        let contexts = Contexts::new(&keyring, KEYRING_SERVICE);
        let err = contexts.save("a b", "http://a", "k").unwrap_err();
        assert_eq!(err, "context name \"a b\" must match \"^[A-Za-z0-9_.-]+$\"");
    }

    #[test]
    fn resolve_prefers_the_flag_then_the_current_context() {
        let keyring = MemoryKeyring::default();
        let contexts = Contexts::new(&keyring, KEYRING_SERVICE);
        contexts.save("dev", "http://dev", "dev-key").unwrap();
        let resolved = resolve(
            &contexts,
            Credentials {
                endpoint: "http://flag".to_owned(),
                api_key: String::new(),
            },
            "",
        )
        .unwrap();
        assert_eq!(
            resolved,
            Credentials {
                endpoint: "http://flag".to_owned(),
                api_key: "dev-key".to_owned()
            }
        );
    }
}
