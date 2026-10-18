// Ports: github.com/expr-lang/expr/parser/operator/operator.go @ v1.17.8

//! The operator tables.

/// How operators of the same precedence group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Associativity {
    /// Go's zero value, which looking up an operator that is not in a table
    /// gives.
    #[default]
    Unset,
    /// `a - b - c` is `(a - b) - c`.
    Left,
    /// `a ** b ** c` is `a ** (b ** c)`.
    Right,
}

/// An entry of the operator tables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Operator {
    /// Higher binds tighter.
    pub precedence: i32,
    /// How a run of operators of this precedence groups.
    pub associativity: Associativity,
}

const fn op(precedence: i32, associativity: Associativity) -> Operator {
    Operator {
        precedence,
        associativity,
    }
}

const UNARY: [(&str, Operator); 4] = [
    ("not", op(50, Associativity::Left)),
    ("!", op(50, Associativity::Left)),
    ("-", op(90, Associativity::Left)),
    ("+", op(90, Associativity::Left)),
];

const BINARY: [(&str, Operator); 25] = [
    ("|", op(0, Associativity::Left)),
    ("or", op(10, Associativity::Left)),
    ("||", op(10, Associativity::Left)),
    ("and", op(15, Associativity::Left)),
    ("&&", op(15, Associativity::Left)),
    ("==", op(20, Associativity::Left)),
    ("!=", op(20, Associativity::Left)),
    ("<", op(20, Associativity::Left)),
    (">", op(20, Associativity::Left)),
    (">=", op(20, Associativity::Left)),
    ("<=", op(20, Associativity::Left)),
    ("in", op(20, Associativity::Left)),
    ("matches", op(20, Associativity::Left)),
    ("contains", op(20, Associativity::Left)),
    ("startsWith", op(20, Associativity::Left)),
    ("endsWith", op(20, Associativity::Left)),
    ("..", op(25, Associativity::Left)),
    ("+", op(30, Associativity::Left)),
    ("-", op(30, Associativity::Left)),
    ("*", op(60, Associativity::Left)),
    ("/", op(60, Associativity::Left)),
    ("%", op(60, Associativity::Left)),
    ("**", op(100, Associativity::Right)),
    ("^", op(100, Associativity::Right)),
    ("??", op(500, Associativity::Left)),
];

fn lookup(table: &[(&str, Operator)], name: &str) -> Option<Operator> {
    table.iter().find(|(n, _)| *n == name).map(|&(_, op)| op)
}

/// `Unary[op]`, ok: the unary operator `op`, if it is one.
pub fn unary_op(op: &str) -> Option<Operator> {
    lookup(&UNARY, op)
}

/// `Binary[op]`, ok: the binary operator `op`, if it is one.
pub fn binary_op(op: &str) -> Option<Operator> {
    lookup(&BINARY, op)
}

/// `Unary[op]`: the zero [`Operator`] when `op` is not a unary operator.
pub fn unary(op: &str) -> Operator {
    unary_op(op).unwrap_or_default()
}

/// `Binary[op]`: the zero [`Operator`] when `op` is not a binary operator.
pub fn binary(op: &str) -> Operator {
    binary_op(op).unwrap_or_default()
}

/// `Less`: `a` binds less tightly than `b`.
pub fn less(a: &str, b: &str) -> bool {
    binary(a).precedence < binary(b).precedence
}

/// `IsBoolean`: `and`, `or`, `&&` or `||`.
pub fn is_boolean(op: &str) -> bool {
    matches!(op, "and" | "or" | "&&" | "||")
}

/// `AllowedNegateSuffix`: the operators `not` may precede, as in `not in`.
pub fn allowed_negate_suffix(op: &str) -> bool {
    matches!(
        op,
        "contains" | "matches" | "startsWith" | "endsWith" | "in"
    )
}

/// `IsComparison`: `<`, `>`, `>=` or `<=`, which chain (`a < b < c`).
pub fn is_comparison(op: &str) -> bool {
    matches!(op, "<" | ">" | ">=" | "<=")
}
