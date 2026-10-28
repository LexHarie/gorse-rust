// Ports: github.com/expr-lang/expr/checker/info.go @ v1.17.8 (the parts the
// compiler reads; the typed function table is not ported, see WP-3-6 notes)

//! Which field or method a node names, so the compiler can load it by index.

use crate::ast::{Node, NodeKind};
use crate::nature::Nature;
use crate::ty::Kind;

/// Go's `FieldIndex`: whether `node` names a field of a struct, with the
/// index path of the field.
pub fn field_index(env: &Nature, node: &Node) -> Option<(Vec<usize>, String)> {
    match &node.kind {
        NodeKind::Identifier { value } => {
            env.field_index(value).map(|index| (index, value.clone()))
        }
        NodeKind::Member {
            node: base,
            property,
            ..
        } => {
            let base = base.nature.deref();
            if base.kind() != Kind::Struct {
                return None;
            }
            let NodeKind::String { value } = &property.kind else {
                return None;
            };
            let name = String::from_utf8_lossy(value).into_owned();
            base.field_index(&name).map(|index| (index, name))
        }
        _ => None,
    }
}

/// Go's `MethodIndex`: whether `node` names a method, with its index.
pub fn method_index(env: &Nature, node: &Node) -> Option<(usize, String)> {
    match &node.kind {
        NodeKind::Identifier { value } => {
            if env.kind() != Kind::Struct {
                return None;
            }
            let nt = env.get(value)?;
            if nt.method {
                Some((nt.method_index, value.clone()))
            } else {
                None
            }
        }
        NodeKind::Member {
            node: base,
            property,
            ..
        } => {
            let NodeKind::String { value } = &property.kind else {
                return None;
            };
            let name = String::from_utf8_lossy(value).into_owned();
            let base_ty = base.nature.ty.as_ref()?;
            if base_ty.kind() == Kind::Interface {
                return None;
            }
            let index = base_ty.methods().iter().position(|m| m.name == name)?;
            Some((index, name))
        }
        _ => None,
    }
}
