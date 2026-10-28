// Ports: github.com/expr-lang/expr/conf/config.go @ v1.17.8 (the fields and methods the parser, the checker, the compiler and the VM read)

//! Configuration of a parse, a check and a compile.
//!
//! Go's `conf.Config` also carries the operators and the option functions of
//! `expr.Compile`; Gorse uses only the environment and the defaults of
//! `expr.Compile`, so those are the fields here.

use std::collections::BTreeSet;

use crate::nature::Nature;

/// `conf.DefaultMaxNodes`: the node budget of a parse without a config.
pub const DEFAULT_MAX_NODES: u64 = 10_000;

/// A configuration. [`Default`] is Go's zero `Config{}`, with no budget and
/// none of the switches on; [`Config::create_new`] is `conf.CreateNew()`.
#[derive(Debug, Clone, Default)]
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
    /// The environment the expression runs in: the type of its variables
    /// (Go: `Env`, a map environment).
    pub env: Nature,
    /// Names the environment does not define are errors (Go: `Strict`).
    pub strict: bool,
    /// Whether the optimizer runs (Go: `Optimize`).
    pub optimize: bool,
    /// Whether `and` and `or` short-circuit (Go: `ShortCircuit`).
    pub short_circuit: bool,
}

impl Config {
    /// `conf.CreateNew()`: the default node budget and the switches on. Not
    /// strict until an environment is set, as in Go.
    pub fn create_new() -> Self {
        Self {
            max_nodes: DEFAULT_MAX_NODES,
            optimize: true,
            short_circuit: true,
            ..Self::default()
        }
    }

    /// Go's `WithEnv` for a map environment: the environment is set, and names
    /// it does not define are errors (Go: `Strict`).
    pub fn with_env(&mut self, env: Nature) {
        self.env = env;
        self.strict = true;
    }

    /// Whether a user function or the environment defines `name`, so a call
    /// of it is not a builtin (Go: `IsOverridden`).
    pub fn is_overridden(&self, name: &str) -> bool {
        self.functions.contains(name) || self.env.get(name).is_some()
    }
}
