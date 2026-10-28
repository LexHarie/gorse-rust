// Ports: github.com/expr-lang/expr/ast/{visitor,find}.go @ v1.17.8

//! Walking a tree.

use super::{Node, NodeKind, with_stack};

/// Something [`walk`] shows every node to.
pub trait Visitor {
    /// Called for each node after its children; it may replace the node.
    fn visit(&mut self, node: &mut Node);
}

/// `ast.Walk`: shows `v` every node, children first, in field order. A
/// builtin's optimizer field `map` is not walked, as in Go.
pub fn walk(node: &mut Node, v: &mut dyn Visitor) {
    with_stack(|| {
        match &mut node.kind {
            NodeKind::Nil
            | NodeKind::Identifier { .. }
            | NodeKind::Integer { .. }
            | NodeKind::Float { .. }
            | NodeKind::Bool { .. }
            | NodeKind::String { .. }
            | NodeKind::Bytes { .. }
            | NodeKind::Pointer { .. }
            | NodeKind::Constant { .. } => {}
            NodeKind::Unary { node, .. }
            | NodeKind::Chain { node }
            | NodeKind::Predicate { node } => walk(node, v),
            NodeKind::Binary { left, right, .. } => {
                walk(left, v);
                walk(right, v);
            }
            NodeKind::Member { node, property, .. } => {
                walk(node, v);
                walk(property, v);
            }
            NodeKind::Slice { node, from, to } => {
                walk(node, v);
                if let Some(from) = from {
                    walk(from, v);
                }
                if let Some(to) = to {
                    walk(to, v);
                }
            }
            NodeKind::Call { callee, arguments } => {
                walk(callee, v);
                for argument in arguments {
                    walk(argument, v);
                }
            }
            NodeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    walk(argument, v);
                }
            }
            NodeKind::VariableDeclarator { value, expr, .. } => {
                walk(value, v);
                walk(expr, v);
            }
            NodeKind::Sequence { nodes }
            | NodeKind::Array { nodes }
            | NodeKind::Map { pairs: nodes } => {
                for node in nodes {
                    walk(node, v);
                }
            }
            NodeKind::Conditional {
                cond, exp1, exp2, ..
            } => {
                walk(cond, v);
                walk(exp1, v);
                walk(exp2, v);
            }
            NodeKind::Pair { key, value } => {
                walk(key, v);
                walk(value, v);
            }
        }
        v.visit(node);
    });
}

/// [`walk`]'s order, without changing the tree.
fn walk_ref<'a>(node: &'a Node, f: &mut dyn FnMut(&'a Node)) {
    with_stack(|| {
        match &node.kind {
            NodeKind::Nil
            | NodeKind::Identifier { .. }
            | NodeKind::Integer { .. }
            | NodeKind::Float { .. }
            | NodeKind::Bool { .. }
            | NodeKind::String { .. }
            | NodeKind::Bytes { .. }
            | NodeKind::Pointer { .. }
            | NodeKind::Constant { .. } => {}
            NodeKind::Unary { node, .. }
            | NodeKind::Chain { node }
            | NodeKind::Predicate { node } => walk_ref(node, f),
            NodeKind::Binary { left, right, .. } => {
                walk_ref(left, f);
                walk_ref(right, f);
            }
            NodeKind::Member { node, property, .. } => {
                walk_ref(node, f);
                walk_ref(property, f);
            }
            NodeKind::Slice { node, from, to } => {
                walk_ref(node, f);
                if let Some(from) = from {
                    walk_ref(from, f);
                }
                if let Some(to) = to {
                    walk_ref(to, f);
                }
            }
            NodeKind::Call { callee, arguments } => {
                walk_ref(callee, f);
                for argument in arguments {
                    walk_ref(argument, f);
                }
            }
            NodeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    walk_ref(argument, f);
                }
            }
            NodeKind::VariableDeclarator { value, expr, .. } => {
                walk_ref(value, f);
                walk_ref(expr, f);
            }
            NodeKind::Sequence { nodes }
            | NodeKind::Array { nodes }
            | NodeKind::Map { pairs: nodes } => {
                for node in nodes {
                    walk_ref(node, f);
                }
            }
            NodeKind::Conditional {
                cond, exp1, exp2, ..
            } => {
                walk_ref(cond, f);
                walk_ref(exp1, f);
                walk_ref(exp2, f);
            }
            NodeKind::Pair { key, value } => {
                walk_ref(key, f);
                walk_ref(value, f);
            }
        }
        f(node);
    });
}

/// `ast.Find`: the last node, in [`walk`] order, that `f` accepts.
pub fn find(node: &Node, f: impl Fn(&Node) -> bool) -> Option<&Node> {
    let mut found = None;
    walk_ref(node, &mut |n| {
        if f(n) {
            found = Some(n);
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(value: &str) -> Node {
        Node::new(NodeKind::Identifier {
            value: value.to_owned(),
        })
    }

    fn sum() -> Node {
        Node::new(NodeKind::Binary {
            operator: "+".to_owned(),
            left: Box::new(ident("foo")),
            right: Box::new(ident("bar")),
        })
    }

    // ast/visitor_test.go
    #[test]
    fn walk_visits_children_in_order() {
        struct Identifiers(Vec<String>);
        impl Visitor for Identifiers {
            fn visit(&mut self, node: &mut Node) {
                if let NodeKind::Identifier { value } = &node.kind {
                    self.0.push(value.clone());
                }
            }
        }
        let mut node = sum();
        let mut v = Identifiers(vec![]);
        walk(&mut node, &mut v);
        assert_eq!(v.0, ["foo", "bar"]);
    }

    #[test]
    fn walk_patch() {
        struct Patcher;
        impl Visitor for Patcher {
            fn visit(&mut self, node: &mut Node) {
                if let NodeKind::Identifier { .. } = node.kind {
                    *node = Node::new(NodeKind::Nil);
                }
            }
        }
        let mut node = sum();
        walk(&mut node, &mut Patcher);
        let NodeKind::Binary { left, right, .. } = &node.kind else {
            panic!("not binary: {node:?}");
        };
        assert_eq!(left.kind, NodeKind::Nil);
        assert_eq!(right.kind, NodeKind::Nil);
    }

    // ast/find_test.go
    #[test]
    fn find_returns_the_matching_node() {
        let root = sum();
        let x = find(
            &root,
            |n| matches!(&n.kind, NodeKind::Identifier { value } if value == "foo"),
        );
        let NodeKind::Binary { left, .. } = &root.kind else {
            panic!("not binary");
        };
        assert!(std::ptr::eq(x.expect("found"), &**left));
    }
}
