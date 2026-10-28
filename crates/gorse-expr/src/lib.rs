// Ports: github.com/expr-lang/expr (file/, ast/, parser/, and the parts of builtin/ and conf/ the parser reads) @ v1.17.8

//! The expression language of Gorse's item expressions: a port of
//! [expr-lang](https://github.com/expr-lang/expr) v1.17.8, the version the Go
//! module pins.
//!
//! Gorse validates the `item_expr` config fields by parsing them
//! (`config/config.go:970-977`): a field is valid when [`parser::parse`]
//! returns no error. The recommenders in `logics` also compile and run
//! expressions; the checker, compiler and VM for that are WP-3-6 and will be
//! added to this crate on top of the parser.
//!
//! ```
//! use gorse_expr::parser;
//!
//! let tree = parser::parse("count(feedback, .FeedbackType == 'star')").expect("valid");
//! assert_eq!(tree.node.to_string(), r#"count(feedback, .FeedbackType == "star")"#);
//!
//! let err = parser::parse("item.Labels.").expect_err("invalid");
//! assert_eq!(err.to_string(), "unexpected end of expression (1:12)\n | item.Labels.\n | ...........^");
//! ```
//!
//! Every module names the Go files it ports; behaviour, error texts and
//! printed trees follow Go exactly, quirks included (the tests compare them
//! with what Go gives for a large corpus). The deliberate differences are
//! listed in `PORTING_NOTES.md`.
//!
//! expr-lang is Copyright (c) 2018 Anton Medvedev and is distributed under the
//! MIT licence, reproduced in `LICENSE-expr-lang`.

pub mod ast;
pub mod builtin;
mod checker;
mod compiler;
pub mod conf;
pub mod file;
mod gostr;
pub mod nature;
mod optimizer;
pub mod parser;
mod regexp;
pub mod ty;
mod unicode_tables;
pub mod value;
pub mod vm;

use conf::Config;
use vm::Program;

/// `expr.Compile`: parses, checks and compiles an expression under a config.
///
/// The config's environment is the type of the variables the expression may
/// name, as Go's `expr.Env` gives it.
pub fn compile(input: &str, config: &Config) -> Result<Program, file::Error> {
    let mut tree = parser::parse_with_config(input, Some(config))?;
    checker::check(&mut tree.node, &tree.source, config)?;
    if config.optimize {
        optimizer::optimize(&mut tree.node, &tree.source)?;
    }
    compiler::compile(&tree.node, &tree.source, config)
}

#[cfg(test)]
mod tests {
    use crate::ty::Ty;
    use crate::value::Value;
    use crate::vm::Program;

    /// The compiled programs and the values they run on are `Send`, so the
    /// logics can move them between threads (D-227). Checked when this compiles.
    #[test]
    fn programs_and_values_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Program>();
        assert_send::<Value>();
        assert_send::<Ty>();
    }
}
