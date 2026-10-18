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
//! listed in `porting/notes/WP-1-19.md`.
//!
//! expr-lang is Copyright (c) 2018 Anton Medvedev and is distributed under the
//! MIT licence, reproduced in `LICENSE-expr-lang`.

pub mod ast;
pub mod builtin;
pub mod conf;
pub mod file;
mod gostr;
pub mod parser;
mod unicode_tables;
