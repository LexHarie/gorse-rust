// Ports: github.com/expr-lang/expr/parser/parser.go @ v1.17.8

//! The parser: a precedence-climbing parser over the lexer's tokens.
//!
//! The functions follow Go's one for one, including what they do after an
//! error: Go keeps going, reading tokens, until the error stops a loop, and
//! a lexer error met on the way replaces the parser's (Q-WP119-2). Where Go
//! returns a `nil` node these return `None`; a `None` child means an error
//! is set, so no node with a missing child is ever made.

pub mod lexer;
pub mod operator;
pub mod utils;

use crate::ast::{Node, NodeKind, with_stack};
use crate::builtin;
use crate::conf::{Config, DEFAULT_MAX_NODES};
use crate::file::{Error, Location, Source};
use crate::gostr;
use crate::nature::Nature;
use lexer::{Kind, Lexer, Token};
use operator::Associativity;

/// What a predicate builtin takes at an argument position.
type Arg = u8;
const EXPR: Arg = 1;
const PREDICATE: Arg = 1 << 1;
const OPTIONAL: Arg = 1 << 7;

/// `predicates`: the builtins whose arguments include a predicate, and what
/// each argument is.
fn predicate_args(name: &str) -> Option<&'static [Arg]> {
    const EP: &[Arg] = &[EXPR, PREDICATE];
    const EP_OPT: &[Arg] = &[EXPR, PREDICATE | OPTIONAL];
    const EPE_OPT: &[Arg] = &[EXPR, PREDICATE, EXPR | OPTIONAL];
    Some(match name {
        "all" | "none" | "any" | "one" | "filter" | "map" | "find" | "findIndex" | "findLast"
        | "findLastIndex" | "groupBy" => EP,
        "count" | "sum" => EP_OPT,
        "sortBy" | "reduce" => EPE_OPT,
        _ => return None,
    })
}

/// A parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    /// The root of the tree.
    pub node: Node,
    /// The text it was parsed from.
    pub source: Source,
}

/// `Parse`: parses `input` with the default configuration (Go: a `nil`
/// config, which allows [`DEFAULT_MAX_NODES`] nodes).
pub fn parse(input: &str) -> Result<Tree, Error> {
    parse_with_config(input, None)
}

/// `ParseWithConfig`: parses `input`. The error, if any, is bound to the
/// source, so it shows where it is.
///
/// Go returns the half-built tree along with an error; nothing reads it, so
/// here an error is all there is.
pub fn parse_with_config(input: &str, config: Option<&Config>) -> Result<Tree, Error> {
    let source = Source::new(input);
    let mut lexer = Lexer::new(&source);
    // propagate config flags to lexer
    lexer.disable_if_operator = config.is_some_and(|c| c.disable_if_operator);
    let mut p = Parser {
        lexer,
        // Go starts from the zero token; it is only looked at once the first
        // read has failed and the error is set.
        current: Token {
            location: Location::default(),
            kind: Kind::Eof,
            value: Vec::new(),
        },
        stashed: None,
        err: None,
        config,
        depth: 0,
        node_count: 0,
    };
    p.next();
    let node = p.parse_sequence_expression();

    if !p.current.is(Kind::Eof, &[]) {
        p.unexpected_token();
    }

    let err = p.err.take();
    drop(p);
    match (err, node) {
        (Some(err), _) => Err(err.bind(&source)),
        (None, Some(node)) => Ok(Tree { node, source }),
        // A node is missing only when an error is set.
        (None, None) => {
            Err(Error::new(Location::default(), "unexpected end of expression").bind(&source))
        }
    }
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    /// Go: `stashed` when `hasStash`.
    stashed: Option<Token>,
    err: Option<Error>,
    config: Option<&'a Config>,
    /// predicate call depth
    depth: usize,
    /// tracks number of AST nodes created
    node_count: u64,
}

impl Parser<'_> {
    fn check_node_limit(&mut self) {
        self.node_count += 1;
        let limit = match self.config {
            None => DEFAULT_MAX_NODES,
            Some(config) if config.max_nodes > 0 => config.max_nodes,
            Some(_) => return,
        };
        if self.node_count > limit {
            self.error(|_| {
                "compilation failed: expression exceeds maximum allowed nodes".to_owned()
            });
        }
    }

    /// `createNode`: counts the node against the budget, then builds it
    /// unless an error is set. `build` gives `None` only for a missing
    /// child, which an error always comes with.
    fn create_node(
        &mut self,
        location: Location,
        build: impl FnOnce() -> Option<NodeKind>,
    ) -> Option<Node> {
        self.check_node_limit();
        if self.err.is_some() {
            return None;
        }
        build().map(|kind| Node {
            location,
            kind,
            nature: Nature::default(),
        })
    }

    fn error(&mut self, message: impl FnOnce(&Self) -> String) {
        self.error_at(self.current.location, message);
    }

    /// `errorAt`: records the first error.
    fn error_at(&mut self, location: Location, message: impl FnOnce(&Self) -> String) {
        if self.err.is_none() {
            // show first error
            let message = message(self);
            self.err = Some(Error::new(location, message));
        }
    }

    fn unexpected_token(&mut self) {
        self.error(|p| format!("unexpected token {}", p.current));
    }

    fn next(&mut self) {
        if let Some(token) = self.stashed.take() {
            self.current = token;
            return;
        }
        match self.lexer.next_token() {
            Ok(Some(token)) => self.current = token,
            Ok(None) => self.error(|_| "unexpected end of expression".to_owned()),
            // Not first-wins, unlike the parser's own errors.
            Err(err) => self.err = Some(err),
        }
    }

    fn expect(&mut self, kind: Kind, values: &[&str]) {
        if self.current.is(kind, values) {
            self.next();
            return;
        }
        self.unexpected_token();
    }

    fn current_str(&self) -> &str {
        self.current.value_str().unwrap_or_default()
    }

    // parse functions

    fn parse_sequence_expression(&mut self) -> Option<Node> {
        let mut nodes = vec![self.parse_expression(0)];

        while self.current.is(Kind::Operator, &[";"]) && self.err.is_none() {
            self.next();
            // If a trailing semicolon is present, break out.
            if self.current.is(Kind::Eof, &[]) {
                break;
            }
            nodes.push(self.parse_expression(0));
        }

        if nodes.len() == 1 {
            return nodes.pop().flatten();
        }

        let location = nodes[0].as_ref().map(|n| n.location).unwrap_or_default();
        self.create_node(location, || {
            Some(NodeKind::Sequence {
                nodes: nodes.into_iter().collect::<Option<_>>()?,
            })
        })
    }

    fn parse_expression(&mut self, precedence: i32) -> Option<Node> {
        with_stack(|| self.parse_expression_inner(precedence))
    }

    fn parse_expression_inner(&mut self, precedence: i32) -> Option<Node> {
        if self.err.is_some() {
            return None;
        }

        if precedence == 0 && self.current.is(Kind::Operator, &["let"]) {
            return self.parse_variable_declaration();
        }

        if precedence == 0
            && self.config.is_none_or(|c| !c.disable_if_operator)
            && self.current.is(Kind::Operator, &["if"])
        {
            return self.parse_conditional_if();
        }

        let mut node_left = self.parse_primary();

        let mut prev_operator: Vec<u8> = Vec::new();
        let mut op_token = self.current.clone();
        while op_token.is(Kind::Operator, &[]) && self.err.is_none() {
            let negate = op_token.is(Kind::Operator, &["not"]);
            let mut not_location = Location::default();

            // Handle "not *" operator, like "not in" or "not contains".
            if negate {
                let token_backup = self.current.clone();
                self.next();
                if operator::allowed_negate_suffix(self.current_str()) {
                    if operator::binary_op(self.current_str())
                        .is_some_and(|op| op.precedence >= precedence)
                    {
                        not_location = self.current.location;
                        op_token = self.current.clone();
                    } else {
                        self.stashed = Some(std::mem::replace(&mut self.current, token_backup));
                        break;
                    }
                } else {
                    self.unexpected_token();
                    break;
                }
            }

            let op_value = op_token.value_str().unwrap_or_default();
            let Some(op) = operator::binary_op(op_value).filter(|op| op.precedence >= precedence)
            else {
                break;
            };
            self.next();

            if op_value == "|" {
                let ident_token = self.current.clone();
                self.expect(Kind::Identifier, &[]);
                node_left = self.parse_call(&ident_token, vec![node_left], true);
            } else if prev_operator == b"??"
                && op_value != "??"
                && !op_token.is(Kind::Bracket, &["("])
            {
                self.error_at(op_token.location, |_| {
                    format!(
                        "Operator ({op_value}) and coalesce expressions (??) cannot be mixed. Wrap either by parentheses."
                    )
                });
                break;
            } else if operator::is_comparison(op_value) {
                node_left = self.parse_comparison(node_left, op_token.clone(), op.precedence);
            } else {
                let node_right = if op.associativity == Associativity::Left {
                    self.parse_expression(op.precedence + 1)
                } else {
                    self.parse_expression(op.precedence)
                };

                node_left = self.create_node(op_token.location, || {
                    Some(NodeKind::Binary {
                        operator: op_value.to_owned(),
                        left: Box::new(node_left?),
                        right: Box::new(node_right?),
                    })
                });
                node_left.as_ref()?;

                if negate {
                    node_left = self.create_node(not_location, || {
                        Some(NodeKind::Unary {
                            operator: "not".to_owned(),
                            node: Box::new(node_left?),
                        })
                    });
                    node_left.as_ref()?;
                }
            }

            // next:
            prev_operator = op_token.value.clone();
            op_token = self.current.clone();
        }

        if precedence == 0 {
            node_left = self.parse_conditional(node_left);
        }

        node_left
    }

    fn parse_variable_declaration(&mut self) -> Option<Node> {
        self.expect(Kind::Operator, &["let"]);
        let variable_name = self.current.clone();
        self.expect(Kind::Identifier, &[]);
        self.expect(Kind::Operator, &["="]);
        let value = self.parse_expression(0);
        self.expect(Kind::Operator, &[";"]);
        let node = self.parse_sequence_expression();
        self.create_node(variable_name.location, || {
            Some(NodeKind::VariableDeclarator {
                name: variable_name.text(),
                value: Box::new(value?),
                expr: Box::new(node?),
            })
        })
    }

    fn parse_conditional_if(&mut self) -> Option<Node> {
        with_stack(|| self.parse_conditional_if_inner())
    }

    fn parse_conditional_if_inner(&mut self) -> Option<Node> {
        self.next();
        if self.err.is_some() {
            return None;
        }
        let node_condition = self.parse_expression(0);
        self.expect(Kind::Bracket, &["{"]);
        let expr1 = self.parse_sequence_expression();
        self.expect(Kind::Bracket, &["}"]);
        self.expect(Kind::Operator, &["else"]);

        let expr2 = if self.current.is(Kind::Operator, &["if"]) {
            self.parse_conditional_if()
        } else {
            self.expect(Kind::Bracket, &["{"]);
            let expr2 = self.parse_sequence_expression();
            self.expect(Kind::Bracket, &["}"]);
            expr2
        };

        // Go makes this node directly, not with createNode: it is not
        // counted, and keeps the zero location (Q-WP119-3).
        Some(Node::new(NodeKind::Conditional {
            ternary: false,
            cond: Box::new(node_condition?),
            exp1: Box::new(expr1?),
            exp2: Box::new(expr2?),
        }))
    }

    fn parse_conditional(&mut self, node: Option<Node>) -> Option<Node> {
        let mut node = node;
        while self.current.is(Kind::Operator, &["?"]) && self.err.is_none() {
            self.next();

            // `a ?: b` shares `a` as condition and first branch.
            let (expr1, expr2, shared);
            if !self.current.is(Kind::Operator, &[":"]) {
                expr1 = self.parse_expression(0);
                self.expect(Kind::Operator, &[":"]);
                expr2 = self.parse_expression(0);
                shared = false;
            } else {
                self.next();
                expr1 = None;
                expr2 = self.parse_expression(0);
                shared = true;
            }

            let location = self.current.location;
            node = self.create_node(location, || {
                let cond = node?;
                let exp1 = if shared { cond.clone() } else { expr1? };
                Some(NodeKind::Conditional {
                    ternary: true,
                    cond: Box::new(cond),
                    exp1: Box::new(exp1),
                    exp2: Box::new(expr2?),
                })
            });
            node.as_ref()?;
        }
        node
    }

    fn parse_primary(&mut self) -> Option<Node> {
        let token = self.current.clone();

        if token.is(Kind::Operator, &[])
            && let Some(op) = token.value_str().and_then(operator::unary_op)
        {
            self.next();
            let expr = self.parse_expression(op.precedence);
            let node = self.create_node(token.location, || {
                Some(NodeKind::Unary {
                    operator: token.text(),
                    node: Box::new(expr?),
                })
            });
            node.as_ref()?;
            return self.parse_postfix_expression(node);
        }

        if token.is(Kind::Bracket, &["("]) {
            self.next();
            let expr = self.parse_sequence_expression();
            self.expect(Kind::Bracket, &[")"]); // "an opened parenthesis is not properly closed"
            return self.parse_postfix_expression(expr);
        }

        if self.depth > 0 && (token.is(Kind::Operator, &["#"]) || token.is(Kind::Operator, &["."]))
        {
            let mut name = String::new();
            if token.is(Kind::Operator, &["#"]) {
                self.next();
                if self.current.is(Kind::Identifier, &[]) {
                    name = self.current.text();
                    self.next();
                }
            }
            let node = self.create_node(token.location, || Some(NodeKind::Pointer { name }));
            node.as_ref()?;
            return self.parse_postfix_expression(node);
        }

        if token.is(Kind::Operator, &["::"]) {
            self.next();
            let token = self.current.clone();
            self.expect(Kind::Identifier, &[]);
            let call = self.parse_call(&token, Vec::new(), false);
            return self.parse_postfix_expression(call);
        }

        self.parse_secondary()
    }

    fn parse_secondary(&mut self) -> Option<Node> {
        let token = self.current.clone();
        let location = token.location;

        let node = match token.kind {
            Kind::Identifier => {
                self.next();
                // `true`, `false` and `nil` take no postfix (Q-WP119-4).
                match token.value.as_slice() {
                    b"true" => {
                        return self.create_node(location, || Some(NodeKind::Bool { value: true }));
                    }
                    b"false" => {
                        return self
                            .create_node(location, || Some(NodeKind::Bool { value: false }));
                    }
                    b"nil" => return self.create_node(location, || Some(NodeKind::Nil)),
                    _ => {
                        if self.current.is(Kind::Bracket, &["("]) {
                            self.parse_call(&token, Vec::new(), true)
                        } else {
                            let node = self.create_node(location, || {
                                Some(NodeKind::Identifier {
                                    value: token.text(),
                                })
                            });
                            node.as_ref()?;
                            node
                        }
                    }
                }
            }

            Kind::Number => {
                self.next();
                // A number takes no postfix either.
                return self.parse_number(&token);
            }

            Kind::String => {
                self.next();
                let node =
                    self.create_node(location, || Some(NodeKind::String { value: token.value }));
                node.as_ref()?;
                node
            }

            Kind::Bytes => {
                self.next();
                let node =
                    self.create_node(location, || Some(NodeKind::Bytes { value: token.value }));
                node.as_ref()?;
                node
            }

            _ => {
                if token.is(Kind::Bracket, &["["]) {
                    self.parse_array_expression(&token)
                } else if token.is(Kind::Bracket, &["{"]) {
                    self.parse_map_expression(&token)
                } else {
                    self.error(|_| format!("unexpected token {token}"));
                    None
                }
            }
        };

        self.parse_postfix_expression(node)
    }

    /// The number case of `parseSecondary`.
    fn parse_number(&mut self, token: &Token) -> Option<Node> {
        // Number tokens are ASCII.
        let value = token.text().replace('_', "");
        let value_lower = value.to_ascii_lowercase();
        let integer = |p: &mut Self, what: &str, base0: bool| match gostr::parse_int(&value, base0)
        {
            Ok(number) => number,
            Err(err) => {
                p.error(|_| format!("invalid {what} literal: {err}"));
                0
            }
        };
        let node = if value_lower.starts_with("0x") {
            let number = integer(self, "hex", true);
            self.integer_node(number)
        } else if value_lower.contains(['.', 'e']) {
            let number = match gostr::parse_float(&value) {
                Ok(number) => number,
                Err(err) => {
                    self.error(|_| format!("invalid float literal: {err}"));
                    // Go's value for a range error; nothing reads it after
                    // the error.
                    f64::INFINITY
                }
            };
            self.float_node(number)
        } else if value_lower.starts_with("0b") {
            let number = integer(self, "binary", true);
            self.integer_node(number)
        } else if value_lower.starts_with("0o") {
            let number = integer(self, "octal", true);
            self.integer_node(number)
        } else {
            let number = integer(self, "integer", false);
            self.integer_node(number)
        };
        node.map(|mut node| {
            node.location = token.location;
            node
        })
    }

    fn integer_node(&mut self, number: i64) -> Option<Node> {
        // Go's int is 64 bits, so `number > math.MaxInt` never holds.
        let location = self.current.location;
        self.create_node(location, || Some(NodeKind::Integer { value: number }))
    }

    fn float_node(&mut self, number: f64) -> Option<Node> {
        if number > f64::MAX {
            self.error(|_| "float literal is too large".to_owned());
            return None;
        }
        let location = self.current.location;
        self.create_node(location, || Some(NodeKind::Float { value: number }))
    }

    fn parse_call(
        &mut self,
        token: &Token,
        mut arguments: Vec<Option<Node>>,
        check_overrides: bool,
    ) -> Option<Node> {
        let name = token.value_str().unwrap_or_default();

        let is_overridden = check_overrides && self.config.is_some_and(|c| c.is_overridden(name));

        if let Some(b) = predicate_args(name)
            && !is_overridden
        {
            self.expect(Kind::Bracket, &["("]);

            // In case of the pipe operator, the first argument is the
            // left-hand side of the operator, so we do not parse it as an
            // argument inside brackets.
            let args = b.get(arguments.len()..).unwrap_or_default();

            for (i, &arg) in args.iter().enumerate() {
                if arg & OPTIONAL == OPTIONAL {
                    if self.current.is(Kind::Bracket, &[")"]) {
                        break;
                    }
                } else if self.current.is(Kind::Bracket, &[")"]) {
                    self.error(|_| format!("expected at least {} arguments", args.len()));
                }

                if i > 0 {
                    self.expect(Kind::Operator, &[","]);
                }
                let node = if arg & EXPR == EXPR {
                    self.parse_expression(0)
                } else {
                    self.parse_predicate()
                };
                arguments.push(node);
            }

            // skip last comma
            if self.current.is(Kind::Operator, &[","]) {
                self.next();
            }
            self.expect(Kind::Bracket, &[")"]);

            self.create_node(token.location, || builtin_node(name, arguments))
        } else if builtin::index(name).is_some()
            && self.config.is_none_or(|c| !c.disabled.contains(name))
            && !is_overridden
        {
            let arguments = self.parse_arguments(arguments);
            self.create_node(token.location, || builtin_node(name, arguments))
        } else {
            let callee = self.create_node(token.location, || {
                Some(NodeKind::Identifier {
                    value: token.text(),
                })
            });
            callee.as_ref()?;
            let arguments = self.parse_arguments(arguments);
            self.create_node(token.location, || {
                Some(NodeKind::Call {
                    callee: Box::new(callee?),
                    arguments: arguments.into_iter().collect::<Option<_>>()?,
                })
            })
        }
    }

    fn parse_arguments(&mut self, mut arguments: Vec<Option<Node>>) -> Vec<Option<Node>> {
        // If pipe operator is used, the first argument is the left-hand side
        // of the operator, so we do not parse it as an argument inside
        // brackets.
        let offset = arguments.len();

        self.expect(Kind::Bracket, &["("]);
        while !self.current.is(Kind::Bracket, &[")"]) && self.err.is_none() {
            if arguments.len() > offset {
                self.expect(Kind::Operator, &[","]);
            }
            if self.current.is(Kind::Bracket, &[")"]) {
                break;
            }
            let node = self.parse_expression(0);
            arguments.push(node);
        }
        self.expect(Kind::Bracket, &[")"]);

        arguments
    }

    fn parse_predicate(&mut self) -> Option<Node> {
        let start_location = self.current.location;
        let with_brackets = self.current.is(Kind::Bracket, &["{"]);
        if with_brackets {
            self.next();
        }

        self.depth += 1;
        let node = if with_brackets {
            self.parse_sequence_expression()
        } else {
            let node = self.parse_expression(0);
            if self.current.is(Kind::Operator, &[";"]) {
                self.error(|_| "wrap predicate with brackets { and }".to_owned());
            }
            node
        };
        self.depth -= 1;

        if with_brackets {
            self.expect(Kind::Bracket, &["}"]);
        }
        self.create_node(start_location, || {
            Some(NodeKind::Predicate {
                node: Box::new(node?),
            })
        })
    }

    fn parse_array_expression(&mut self, token: &Token) -> Option<Node> {
        let mut nodes = Vec::new();

        self.expect(Kind::Bracket, &["["]);
        while !self.current.is(Kind::Bracket, &["]"]) && self.err.is_none() {
            if !nodes.is_empty() {
                self.expect(Kind::Operator, &[","]);
                if self.current.is(Kind::Bracket, &["]"]) {
                    break; // goto end
                }
            }
            let node = self.parse_expression(0);
            nodes.push(node);
        }
        // end:
        self.expect(Kind::Bracket, &["]"]);

        self.create_node(token.location, || {
            Some(NodeKind::Array {
                nodes: nodes.into_iter().collect::<Option<_>>()?,
            })
        })
    }

    fn parse_map_expression(&mut self, token: &Token) -> Option<Node> {
        self.expect(Kind::Bracket, &["{"]);

        let mut nodes = Vec::new();
        while !self.current.is(Kind::Bracket, &["}"]) && self.err.is_none() {
            if !nodes.is_empty() {
                self.expect(Kind::Operator, &[","]);
                if self.current.is(Kind::Bracket, &["}"]) {
                    break; // goto end
                }
                if self.current.is(Kind::Operator, &[","]) {
                    self.unexpected_token();
                }
            }

            // Map key can be one of:
            //  * number
            //  * string
            //  * identifier, which is equivalent to a string
            //  * expression, which must be enclosed in parentheses -- (1 + 2)
            let key = if self.current.is(Kind::Number, &[])
                || self.current.is(Kind::String, &[])
                || self.current.is(Kind::Identifier, &[])
            {
                let key_token = self.current.clone();
                let key = self.create_node(key_token.location, || {
                    Some(NodeKind::String {
                        value: key_token.value,
                    })
                });
                key.as_ref()?;
                self.next();
                key
            } else if self.current.is(Kind::Bracket, &["("]) {
                self.parse_expression(0)
            } else {
                self.error(|p| {
                    format!(
                        "a map key must be a quoted string, a number, a identifier, or an expression enclosed in parentheses (unexpected token {})",
                        p.current
                    )
                });
                None
            };

            self.expect(Kind::Operator, &[":"]);

            let node = self.parse_expression(0);
            // The pair is at the `{` of the map (Q-WP119-5).
            let pair = self.create_node(token.location, || {
                Some(NodeKind::Pair {
                    key: Box::new(key?),
                    value: Box::new(node?),
                })
            })?;
            nodes.push(pair);
        }

        // end:
        self.expect(Kind::Bracket, &["}"]);

        self.create_node(token.location, || Some(NodeKind::Map { pairs: nodes }))
    }

    fn parse_postfix_expression(&mut self, node: Option<Node>) -> Option<Node> {
        let mut node = node;
        let mut postfix_token = self.current.clone();
        'postfix: while (postfix_token.is(Kind::Operator, &[])
            || postfix_token.is(Kind::Bracket, &[]))
            && self.err.is_none()
        {
            let optional = postfix_token.value == b"?.";
            // parseToken: `?.[` goes round again as `[`.
            loop {
                if postfix_token.value == b"." || postfix_token.value == b"?." {
                    self.next();

                    let property_token = self.current.clone();
                    if optional && property_token.is(Kind::Bracket, &["["]) {
                        postfix_token = property_token;
                        continue; // goto parseToken
                    }
                    self.next();

                    if property_token.kind != Kind::Identifier
                        // Operators like "not" and "matches" are valid
                        // methods or property names.
                        && (property_token.kind != Kind::Operator
                            || !property_token.value_str().is_some_and(utils::is_valid_identifier))
                    {
                        self.error(|_| "expected name".to_owned());
                    }

                    let property = self.create_node(property_token.location, || {
                        Some(NodeKind::String {
                            value: property_token.value.clone(),
                        })
                    });
                    property.as_ref()?;

                    let is_chain = node
                        .as_ref()
                        .is_some_and(|n| matches!(n.kind, NodeKind::Chain { .. }));
                    let optional = postfix_token.value == b"?.";

                    if is_chain {
                        node = node.map(unwrap_chain);
                    }

                    // createMemberNode
                    let mut member_node = self.create_node(property_token.location, || {
                        Some(NodeKind::Member {
                            node: Box::new(node?),
                            property: Box::new(property?),
                            optional,
                            method: false,
                        })
                    })?;

                    if self.current.is(Kind::Bracket, &["("]) {
                        if let NodeKind::Member { method, .. } = &mut member_node.kind {
                            *method = true;
                        }
                        let arguments = self.parse_arguments(Vec::new());
                        node = self.create_node(property_token.location, || {
                            Some(NodeKind::Call {
                                callee: Box::new(member_node),
                                arguments: arguments.into_iter().collect::<Option<_>>()?,
                            })
                        });
                        node.as_ref()?;
                    } else {
                        node = Some(member_node);
                    }

                    if is_chain || optional {
                        node = self.create_node(property_token.location, || {
                            Some(NodeKind::Chain {
                                node: Box::new(node?),
                            })
                        });
                        node.as_ref()?;
                    }
                } else if postfix_token.value == b"[" {
                    self.next();

                    if self.current.is(Kind::Operator, &[":"]) {
                        // slice without from [:1]
                        self.next();

                        let to = if !self.current.is(Kind::Bracket, &["]"]) {
                            // slice without from and to [:]
                            self.parse_expression(0)
                        } else {
                            None
                        };

                        node = self.create_node(postfix_token.location, || {
                            Some(NodeKind::Slice {
                                node: Box::new(node?),
                                from: None,
                                to: to.map(Box::new),
                            })
                        });
                        node.as_ref()?;
                        self.expect(Kind::Bracket, &["]"]);
                    } else {
                        let from = self.parse_expression(0);

                        if self.current.is(Kind::Operator, &[":"]) {
                            self.next();

                            let to = if !self.current.is(Kind::Bracket, &["]"]) {
                                // slice without to [1:]
                                self.parse_expression(0)
                            } else {
                                None
                            };

                            node = self.create_node(postfix_token.location, || {
                                Some(NodeKind::Slice {
                                    node: Box::new(node?),
                                    from: Some(Box::new(from?)),
                                    to: to.map(Box::new),
                                })
                            });
                            node.as_ref()?;
                            self.expect(Kind::Bracket, &["]"]);
                        } else {
                            // Slice operator [:] was not found, it should be
                            // just an index node.
                            node = self.create_node(postfix_token.location, || {
                                Some(NodeKind::Member {
                                    node: Box::new(node?),
                                    property: Box::new(from?),
                                    optional,
                                    method: false,
                                })
                            });
                            node.as_ref()?;
                            if optional {
                                node = self.create_node(postfix_token.location, || {
                                    Some(NodeKind::Chain {
                                        node: Box::new(node?),
                                    })
                                });
                                node.as_ref()?;
                            }
                            self.expect(Kind::Bracket, &["]"]);
                        }
                    }
                } else {
                    break 'postfix;
                }
                break;
            }
            postfix_token = self.current.clone();
        }
        node
    }

    fn parse_comparison(
        &mut self,
        left: Option<Node>,
        token: Token,
        precedence: i32,
    ) -> Option<Node> {
        let mut left = left;
        let mut token = token;
        let mut root_node: Option<Node> = None;
        loop {
            let comparator = self.parse_expression(precedence + 1);
            // `a < b < c` is `a < b && b < c`, sharing `b`.
            let cmp_node = self.create_node(token.location, || {
                Some(NodeKind::Binary {
                    operator: token.text(),
                    left: Box::new(left?),
                    right: Box::new(comparator.clone()?),
                })
            })?;
            root_node = Some(match root_node {
                None => cmp_node,
                Some(root) => self.create_node(token.location, || {
                    Some(NodeKind::Binary {
                        operator: "&&".to_owned(),
                        left: Box::new(root),
                        right: Box::new(cmp_node),
                    })
                })?,
            });

            left = comparator;
            token = self.current.clone();
            if !(token.is(Kind::Operator, &[])
                && token.value_str().is_some_and(operator::is_comparison)
                && self.err.is_none())
            {
                break;
            }
            self.next();
        }
        root_node
    }
}

fn builtin_node(name: &str, arguments: Vec<Option<Node>>) -> Option<NodeKind> {
    Some(NodeKind::Builtin {
        name: name.to_owned(),
        arguments: arguments.into_iter().collect::<Option<_>>()?,
        throws: false,
        map: None,
        threshold: None,
    })
}

/// The node of a [`NodeKind::Chain`].
fn unwrap_chain(mut chain: Node) -> Node {
    match std::mem::replace(&mut chain.kind, NodeKind::Nil) {
        NodeKind::Chain { node } => *node,
        kind => {
            chain.kind = kind;
            chain
        }
    }
}
