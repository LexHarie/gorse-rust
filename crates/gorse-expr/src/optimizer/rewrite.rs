// Ports: github.com/expr-lang/expr/optimizer/{in_array,in_range,filter_map,
// filter_len,filter_first,filter_last,predicate_combination,sum_range,
// sum_array,sum_map,count_any,count_threshold}.go @ v1.17.8, with the Gorse
// patches of sum_range.go and count_threshold.go

//! The rewrites of operator and builtin shapes. Each turns a shape into a
//! cheaper one with the same value.

use std::rc::Rc;

use crate::ast::{Node, NodeKind, Visitor, find};
use crate::ty::{Kind, Ty};
use crate::value::Value;

use super::{Rewrite, apply, builtin, constant, integer, is_integer, is_plain_pointer};

/// `inArray`: `x in [1, 2]` tests membership in a set of the constants.
pub struct InArray;

impl Visitor for InArray {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = in_array(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn in_array(node: &Node) -> Option<NodeKind> {
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &node.kind
    else {
        return None;
    };
    if operator != "in" {
        return None;
    }
    let NodeKind::Array { nodes } = &right.kind else {
        return None;
    };
    if nodes.is_empty() {
        return None;
    }
    // Go looks an int up in a set of ints only when the left side is an int,
    // since a map lookup needs keys of the checked type; otherwise a set of
    // strings is used when the array holds only strings.
    let (key, keys) = match (
        left.nature.kind() == Kind::Int,
        literal_ints(nodes),
        literal_strings(nodes),
    ) {
        (true, Some(keys), _) => (Ty::int(), keys),
        (_, _, Some(keys)) => (Ty::string(), keys),
        _ => return None,
    };
    let set = Value::Map(
        Ty::map(key, Ty::any()),
        Rc::new(keys.into_iter().map(|key| (key, Value::Nil)).collect()),
    );
    Some(NodeKind::Binary {
        operator: operator.clone(),
        left: left.clone(),
        right: Box::new(constant(set)),
    })
}

fn literal_ints(nodes: &[Node]) -> Option<Vec<Value>> {
    nodes
        .iter()
        .map(|node| match node.kind {
            NodeKind::Integer { value } => Some(Value::Int(value)),
            _ => None,
        })
        .collect()
}

fn literal_strings(nodes: &[Node]) -> Option<Vec<Value>> {
    nodes
        .iter()
        .map(|node| match &node.kind {
            NodeKind::String { value } => Some(Value::Str(Rc::from(value.as_slice()))),
            _ => None,
        })
        .collect()
}

/// `inRange`: `x in a..b` with integer bounds is `x >= a and x <= b`.
pub struct InRange;

impl Visitor for InRange {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = in_range(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn in_range(node: &Node) -> Option<NodeKind> {
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &node.kind
    else {
        return None;
    };
    if operator != "in" || left.nature.kind() != Kind::Int {
        return None;
    }
    let NodeKind::Binary {
        operator: range,
        left: from,
        right: to,
    } = &right.kind
    else {
        return None;
    };
    if range != ".." || integer(from).is_none() || integer(to).is_none() {
        return None;
    }
    Some(NodeKind::Binary {
        operator: "and".to_owned(),
        left: Box::new(compare(">=", left, from)),
        right: Box::new(compare("<=", left, to)),
    })
}

/// `left op right` as a node.
fn compare(operator: &str, left: &Node, right: &Node) -> Node {
    Node::new(NodeKind::Binary {
        operator: operator.to_owned(),
        left: Box::new(left.clone()),
        right: Box::new(right.clone()),
    })
}

/// `filterMap`: `map(filter(xs, p), f)` is a filter that also maps.
pub struct FilterMap;

impl Visitor for FilterMap {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = filter_map(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn filter_map(node: &Node) -> Option<NodeKind> {
    let NodeKind::Builtin {
        name, arguments, ..
    } = &node.kind
    else {
        return None;
    };
    if name != "map" || arguments.len() != 2 || find(&arguments[1], is_index_pointer).is_some() {
        return None;
    }
    let NodeKind::Predicate { node: body } = &arguments[1].kind else {
        return None;
    };
    let NodeKind::Builtin {
        name: inner,
        arguments: filter,
        map: None,
        ..
    } = &arguments[0].kind
    else {
        return None;
    };
    if inner != "filter" {
        return None;
    }
    Some(NodeKind::Builtin {
        name: "filter".to_owned(),
        arguments: filter.clone(),
        throws: false,
        map: Some(body.clone()),
        threshold: None,
    })
}

/// Whether `node` is `#index`.
fn is_index_pointer(node: &Node) -> bool {
    matches!(&node.kind, NodeKind::Pointer { name } if name == "index")
}

/// `filterLen`: `len(filter(xs, p))` is `count(xs, p)`.
pub struct FilterLen;

impl Visitor for FilterLen {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = filter_len(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn filter_len(node: &Node) -> Option<NodeKind> {
    let NodeKind::Builtin {
        name, arguments, ..
    } = &node.kind
    else {
        return None;
    };
    if name != "len" || arguments.len() != 1 {
        return None;
    }
    let (filter, _) = filter_of(&arguments[0])?;
    Some(builtin("count", filter.to_vec()))
}

/// The arguments and the map of a `filter` with its predicate.
fn filter_of(node: &Node) -> Option<(&[Node], &Option<Box<Node>>)> {
    match &node.kind {
        NodeKind::Builtin {
            name,
            arguments,
            map,
            ..
        } if name == "filter" && arguments.len() == 2 => Some((arguments, map)),
        _ => None,
    }
}

/// `filterFirst`: `filter(xs, p)[0]` and `first(filter(xs, p))` find the first
/// match.
pub struct FilterFirst;

impl Visitor for FilterFirst {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = filter_first(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn filter_first(node: &Node) -> Option<NodeKind> {
    match &node.kind {
        // Throws, as indexing an empty result does.
        NodeKind::Member {
            node: target,
            property,
            optional: false,
            ..
        } if is_integer(property, 0) => {
            let (arguments, map) = filter_of(target)?;
            Some(finder("find", arguments, map, true))
        }
        NodeKind::Builtin {
            name, arguments, ..
        } if name == "first" && arguments.len() == 1 => {
            let (filter, map) = filter_of(&arguments[0])?;
            Some(finder("find", filter, map, false))
        }
        _ => None,
    }
}

/// `filterLast`: `filter(xs, p)[-1]` and `last(filter(xs, p))` find the last
/// match.
pub struct FilterLast;

impl Visitor for FilterLast {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = filter_last(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn filter_last(node: &Node) -> Option<NodeKind> {
    match &node.kind {
        NodeKind::Member {
            node: target,
            property,
            optional: false,
            ..
        } if is_integer(property, -1) => {
            let (arguments, map) = filter_of(target)?;
            Some(finder("findLast", arguments, map, true))
        }
        NodeKind::Builtin {
            name, arguments, ..
        } if name == "last" && arguments.len() == 1 => {
            let (filter, map) = filter_of(&arguments[0])?;
            Some(finder("findLast", filter, map, false))
        }
        _ => None,
    }
}

fn finder(name: &str, arguments: &[Node], map: &Option<Box<Node>>, throws: bool) -> NodeKind {
    NodeKind::Builtin {
        name: name.to_owned(),
        arguments: arguments.to_vec(),
        throws,
        map: map.clone(),
        threshold: None,
    }
}

/// `predicateCombination`: `all(xs, p) and all(xs, q)` is `all(xs, p && q)`,
/// and the same for `any` and `none`.
pub struct PredicateCombination;

impl Visitor for PredicateCombination {
    fn visit(&mut self, node: &mut Node) {
        let Some((name, first, mut body)) = combination(node) else {
            return;
        };
        self.visit(&mut body);
        let predicate = Node::new(NodeKind::Predicate {
            node: Box::new(body),
        });
        apply(node, Rewrite::Copy(builtin(&name, vec![first, predicate])));
    }
}

/// The builtin, its first argument, and the combined predicate body, for a
/// boolean of two calls of one builtin over the same collection.
fn combination(node: &Node) -> Option<(String, Node, Node)> {
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &node.kind
    else {
        return None;
    };
    if !matches!(operator.as_str(), "and" | "or" | "&&" | "||") {
        return None;
    }
    let NodeKind::Builtin {
        name,
        arguments: left_args,
        ..
    } = &left.kind
    else {
        return None;
    };
    let combined = combined_operator(name, operator)?;
    let NodeKind::Builtin {
        name: right_name,
        arguments: right_args,
        ..
    } = &right.kind
    else {
        return None;
    };
    if right_name != name {
        return None;
    }
    let (first, second) = (left_args.first()?, right_args.first()?);
    if first.nature.ty != second.nature.ty || first.to_string() != second.to_string() {
        return None;
    }
    let (NodeKind::Predicate { node: l }, NodeKind::Predicate { node: r }) =
        (&left_args.get(1)?.kind, &right_args.get(1)?.kind)
    else {
        return None;
    };
    let body = Node::new(NodeKind::Binary {
        operator: combined,
        left: l.clone(),
        right: r.clone(),
    });
    Some((name.clone(), first.clone(), body))
}

/// The operator that combines the predicates of two builtins joined by
/// `operator`: `none` negates them, so its `and` becomes an `or`.
fn combined_operator(function: &str, operator: &str) -> Option<String> {
    match (function, operator) {
        ("all", "and" | "&&") | ("any", "or" | "||") => Some(operator.to_owned()),
        ("none", "and") => Some("or".to_owned()),
        ("none", "&&") => Some("||".to_owned()),
        _ => None,
    }
}

/// `sumRange` (with Gorse's patches): a sum of an integer range has a closed
/// form, as does a `reduce` of `# + #acc` over one.
pub struct SumRange;

impl Visitor for SumRange {
    fn visit(&mut self, node: &mut Node) {
        if let Some(value) = sum_of_range(node) {
            apply(node, Rewrite::Typed(NodeKind::Integer { value }));
        }
        if let Some(value) = reduce_of_range(node) {
            apply(node, Rewrite::Typed(NodeKind::Integer { value }));
        }
    }
}

/// The sum of `m..n`, or of its values under a predicate when the predicate
/// has a closed form.
fn sum_of_range(node: &Node) -> Option<i64> {
    let NodeKind::Builtin {
        name, arguments, ..
    } = &node.kind
    else {
        return None;
    };
    if name != "sum" || !matches!(arguments.len(), 1 | 2) {
        return None;
    }
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &arguments[0].kind
    else {
        return None;
    };
    if operator != ".." {
        return None;
    }
    let (m, n) = (integer(left)?, integer(right)?);
    if n < m {
        return None;
    }
    let count = n.wrapping_sub(m).wrapping_add(1);
    let sum = count.wrapping_mul(m.wrapping_add(n)) / 2;
    match arguments.get(1) {
        None => Some(sum),
        Some(predicate) => apply_sum_predicate(sum, count, predicate),
    }
}

/// The `reduce` of `# + #acc`, or of `#acc + #`, over `m..n`, with an optional
/// integer initial value.
fn reduce_of_range(node: &Node) -> Option<i64> {
    let NodeKind::Builtin {
        name, arguments, ..
    } = &node.kind
    else {
        return None;
    };
    if name != "reduce" || !matches!(arguments.len(), 2 | 3) {
        return None;
    }
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &arguments[0].kind
    else {
        return None;
    };
    if operator != ".." || !is_pointer_plus_acc(&arguments[1]) {
        return None;
    }
    let (m, n) = (integer(left)?, integer(right)?);
    if n < m {
        return None;
    }
    let sum = n
        .wrapping_sub(m)
        .wrapping_add(1)
        .wrapping_mul(m.wrapping_add(n))
        / 2;
    match arguments.get(2) {
        None => Some(sum),
        Some(initial) => Some(integer(initial)?.wrapping_add(sum)),
    }
}

/// Whether a predicate is `# + #acc` or `#acc + #`.
fn is_pointer_plus_acc(predicate: &Node) -> bool {
    let NodeKind::Predicate { node: body } = &predicate.kind else {
        return false;
    };
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &body.kind
    else {
        return false;
    };
    if operator != "+" {
        return false;
    }
    let (NodeKind::Pointer { name: l }, NodeKind::Pointer { name: r }) = (&left.kind, &right.kind)
    else {
        return false;
    };
    matches!((l.as_str(), r.as_str()), ("", "acc") | ("acc", ""))
}

/// The sum of `m..n` under a predicate of the form `#`, `#*k`, `k*#`, `#+k`,
/// `k+#`, `#-k` or `k-#`, computed from the sum and the count of the range.
fn apply_sum_predicate(sum: i64, count: i64, predicate: &Node) -> Option<i64> {
    let NodeKind::Predicate { node: body } = &predicate.kind else {
        return None;
    };
    if is_plain_pointer(body) {
        return Some(sum);
    }
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &body.kind
    else {
        return None;
    };
    let (k, pointer_on_left) = if is_plain_pointer(left) {
        (integer(right)?, true)
    } else if is_plain_pointer(right) {
        (integer(left)?, false)
    } else {
        return None;
    };
    match operator.as_str() {
        "*" => Some(k.wrapping_mul(sum)),
        "+" => Some(sum.wrapping_add(count.wrapping_mul(k))),
        "-" if pointer_on_left => Some(sum.wrapping_sub(count.wrapping_mul(k))),
        "-" => Some(count.wrapping_mul(k).wrapping_sub(sum)),
        _ => None,
    }
}

/// `sumArray`: a sum of an array literal is the chain of its additions.
pub struct SumArray;

impl Visitor for SumArray {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = sum_array(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn sum_array(node: &Node) -> Option<NodeKind> {
    let NodeKind::Builtin {
        name, arguments, ..
    } = &node.kind
    else {
        return None;
    };
    if name != "sum" || arguments.len() != 1 {
        return None;
    }
    let NodeKind::Array { nodes } = &arguments[0].kind else {
        return None;
    };
    if nodes.len() < 2 {
        return None;
    }
    Some(sum_array_fold(nodes.clone()))
}

/// `a + (b + (c + d))` for the nodes `a, b, c, d`; the caller passes at least
/// two nodes.
fn sum_array_fold(mut nodes: Vec<Node>) -> NodeKind {
    if nodes.len() > 2 {
        let rest = nodes.split_off(1);
        let left = nodes.pop().expect("the first of more than two nodes");
        NodeKind::Binary {
            operator: "+".to_owned(),
            left: Box::new(left),
            right: Box::new(Node::new(sum_array_fold(rest))),
        }
    } else {
        let right = nodes.pop().expect("the second of two nodes");
        let left = nodes.pop().expect("the first of two nodes");
        NodeKind::Binary {
            operator: "+".to_owned(),
            left: Box::new(left),
            right: Box::new(right),
        }
    }
}

/// `sumMap`: `sum(map(xs, f))` is the builtin `sum` over the map's arguments.
pub struct SumMap;

impl Visitor for SumMap {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = sum_map(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn sum_map(node: &Node) -> Option<NodeKind> {
    let NodeKind::Builtin {
        name, arguments, ..
    } = &node.kind
    else {
        return None;
    };
    if name != "sum" || arguments.len() != 1 {
        return None;
    }
    let NodeKind::Builtin {
        name: inner,
        arguments: map,
        ..
    } = &arguments[0].kind
    else {
        return None;
    };
    if inner != "map" || map.len() != 2 {
        return None;
    }
    Some(builtin("sum", map.clone()))
}

/// `countAny`: `count(xs, p) > 0` and `count(xs, p) >= 1` are `any(xs, p)`.
pub struct CountAny;

impl Visitor for CountAny {
    fn visit(&mut self, node: &mut Node) {
        if let Some(kind) = count_any(node) {
            apply(node, Rewrite::Copy(kind));
        }
    }
}

fn count_any(node: &Node) -> Option<NodeKind> {
    let NodeKind::Binary {
        operator,
        left,
        right,
    } = &node.kind
    else {
        return None;
    };
    let NodeKind::Builtin {
        name, arguments, ..
    } = &left.kind
    else {
        return None;
    };
    if name != "count" || arguments.len() != 2 {
        return None;
    }
    let value = integer(right)?;
    if (operator == ">" && value == 0) || (operator == ">=" && value == 1) {
        Some(builtin("any", arguments.clone()))
    } else {
        None
    }
}

/// `countThreshold` (a Gorse patch): a comparison of a count with an integer
/// records the count at which the result is known, so `count` can stop there.
/// The comparison itself is kept.
pub struct CountThreshold;

impl Visitor for CountThreshold {
    fn visit(&mut self, node: &mut Node) {
        let NodeKind::Binary {
            operator,
            left,
            right,
        } = &mut node.kind
        else {
            return;
        };
        let NodeKind::Builtin {
            name,
            arguments,
            threshold,
            ..
        } = &mut left.kind
        else {
            return;
        };
        if name != "count" || arguments.len() != 2 {
            return;
        }
        let Some(value) = integer(right) else {
            return;
        };
        if value < 0 {
            return;
        }
        let limit = match operator.as_str() {
            ">" | "<=" => value.wrapping_add(1),
            ">=" | "<" => value,
            _ => return,
        };
        // A limit of 0 or 1 is `any`, which countAny handles.
        if limit <= 1 {
            return;
        }
        *threshold = Some(limit);
    }
}
