// Ports: github.com/expr-lang/expr/conf/config.go @ v1.17.8 (the fields and methods the parser reads)

//! Parser configuration.
//!
//! Go's `conf.Config` also carries the environment's types, the function
//! table, the builtins and the optimizer and VM switches; the parser reads
//! only what is here. WP-3-6 adds the rest with the checker, compiler and VM.

use std::collections::BTreeSet;

/// `conf.DefaultMaxNodes`: the node budget of a parse without a config.
pub const DEFAULT_MAX_NODES: u64 = 10_000;

/// What the parser reads from a `conf.Config`.
///
/// [`Default`] is Go's zero `Config{}`, whose `max_nodes` of 0 means no
/// budget; [`Config::create_new`] is `conf.CreateNew()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// The most nodes a tree may have; 0 for no limit (Go: `MaxNodes`).
    pub max_nodes: u64,
    /// Builtins parsed as ordinary calls (Go: the keys of `Disabled` that
    /// map to true).
    pub disabled: BTreeSet<String>,
    /// Lex `if` and `else` as identifiers and parse no `if` expressions
    /// (Go: `DisableIfOperator`).
    pub disable_if_operator: bool,
    /// The names of the user's functions (Go: the keys of `Functions`).
    pub functions: BTreeSet<String>,
    /// The names the environment defines (Go: the names `Env.Get` finds).
    /// WP-3-6 derives them from the environment's type.
    pub env: BTreeSet<String>,
}

impl Config {
    /// `conf.CreateNew()`: the default node budget and nothing else set.
    pub fn create_new() -> Self {
        Self {
            max_nodes: DEFAULT_MAX_NODES,
            ..Self::default()
        }
    }

    /// Whether a user function or the environment defines `name`, so a call
    /// of it is not a builtin (Go: `IsOverridden`).
    pub fn is_overridden(&self, name: &str) -> bool {
        self.functions.contains(name) || self.env.contains(name)
    }
}
