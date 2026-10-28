// Ports: github.com/expr-lang/expr/checker/{checker,info}.go @ v1.17.8

//! The checker: it gives every node its nature and rejects expressions that
//! would fail at run time for their types, with the messages Go gives.
//!
//! Each node gets the [`Nature`] of its value, stored on the node. The
//! compiler and the optimizer read those natures. The first error wins, as
//! in Go, and it is reported at its node's location.

mod info;

pub use info::{field_index, method_index};

use crate::ast::{Node, NodeKind};
use crate::builtin::{self, Function};
use crate::conf::Config;
use crate::file::{Error, Location, Source};
use crate::nature::{self, Check, Nature};
use crate::ty::{Kind, Ty};
use crate::value::{duration_type, time_type};

/// A scope of a predicate: the collection it iterates, and the variables
/// the predicate sees (`#index`, `#acc`, ...).
struct PredicateScope {
    collection: Nature,
    vars: Vec<VarScope>,
}

/// A name bound by a `let` or a predicate, with its nature.
#[derive(Clone)]
struct VarScope {
    name: String,
    nature: Nature,
}

/// The checker's state.
struct Checker<'a> {
    config: &'a Config,
    predicate_scopes: Vec<PredicateScope>,
    var_scopes: Vec<VarScope>,
    err: Option<Error>,
}

/// Go's `Check`: the type of the root of `tree` after its nodes are checked.
/// The natures are left on the nodes for the compiler.
pub fn check(node: &mut Node, source: &Source, config: &Config) -> Result<Ty, Error> {
    let mut checker = Checker {
        config,
        predicate_scopes: Vec::new(),
        var_scopes: Vec::new(),
        err: None,
    };
    let nt = checker.visit(node);
    if let Some(err) = checker.err {
        return Err(err.bind(source));
    }
    Ok(nt.ty.unwrap_or_else(Ty::any))
}

/// The error of a builtin's argument check, as `checkArguments` makes it.
type CheckError = Error;

fn from_type(ty: &Ty) -> Nature {
    nature::from_type(ty)
}

fn bool_nature() -> Nature {
    from_type(&Ty::bool())
}

fn int_nature() -> Nature {
    from_type(&Ty::int())
}

fn float_nature() -> Nature {
    from_type(&Ty::float64())
}

fn string_nature() -> Nature {
    from_type(&Ty::string())
}

impl Checker<'_> {
    fn visit(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let nt = match &mut node.kind {
            NodeKind::Nil => nature::nil(),
            NodeKind::Identifier { value } => {
                let name = value.clone();
                self.identifier(location, &name)
            }
            NodeKind::Integer { .. } => int_nature(),
            NodeKind::Float { .. } => float_nature(),
            NodeKind::Bool { .. } => bool_nature(),
            NodeKind::String { .. } => string_nature(),
            NodeKind::Bytes { .. } => from_type(&Ty::byte_slice()),
            NodeKind::Constant { value } => match value.ty() {
                Some(ty) => from_type(&ty),
                None => nature::nil(),
            },
            NodeKind::Unary { .. } => self.unary(node),
            NodeKind::Binary { .. } => self.binary(node),
            NodeKind::Chain { node: inner } => self.visit(inner),
            NodeKind::Member { .. } => self.member(node),
            NodeKind::Slice { .. } => self.slice(node),
            NodeKind::Call { .. } => self.call(node),
            NodeKind::Builtin { .. } => self.builtin(node),
            NodeKind::Predicate { node: inner } => {
                let nt = self.visit(inner);
                let out = if nt.is_unknown() {
                    Some(Ty::any())
                } else if !nt.nil {
                    nt.ty.clone()
                } else {
                    None
                };
                let outs = out.into_iter().collect();
                let mut n = from_type(&Ty::func(vec![Ty::any()], outs, false));
                n.reference = Some(Box::new(nt));
                n
            }
            NodeKind::Pointer { name } => {
                let name = name.clone();
                self.pointer(location, &name)
            }
            NodeKind::Conditional { .. } => self.conditional(node),
            NodeKind::VariableDeclarator { .. } => self.variable_declarator(node),
            NodeKind::Sequence { .. } => self.sequence(node),
            NodeKind::Array { .. } => self.array(node),
            NodeKind::Map { pairs } => {
                for pair in pairs.iter_mut() {
                    self.visit(pair);
                }
                from_type(&Ty::map(Ty::string(), Ty::any()))
            }
            NodeKind::Pair { key, value } => {
                self.visit(key);
                self.visit(value);
                nature::nil()
            }
        };
        node.nature = nt.clone();
        nt
    }

    fn error(&mut self, location: Location, message: impl Into<String>) -> Nature {
        if self.err.is_none() {
            self.err = Some(Error::new(location, message));
        }
        Nature::default()
    }

    fn identifier(&mut self, location: Location, name: &str) -> Nature {
        for scope in self.var_scopes.iter().rev() {
            if scope.name == name {
                return scope.nature.clone();
            }
        }
        if name == "$env" {
            return Nature::default();
        }
        self.ident(location, name, self.config.strict, true)
    }

    fn ident(&mut self, location: Location, name: &str, strict: bool, builtins: bool) -> Nature {
        if let Some(nt) = self.config.env.get(name) {
            return nt;
        }
        if builtins
            && !self.config.disabled.contains(name)
            && let Some(f) = builtin::lookup(name)
        {
            let mut nt = from_type(&f.ty());
            nt.func = Some(f);
            return nt;
        }
        if self.config.strict && strict {
            return self.error(location, format!("unknown name {name}"));
        }
        Nature::default()
    }

    fn unary(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Unary {
            operator,
            node: inner,
        } = &mut node.kind
        else {
            unreachable!("unary node")
        };
        let operator = operator.clone();
        let nt = self.visit(inner).deref();
        match operator.as_str() {
            "!" | "not" => {
                if nt.is_bool() {
                    return bool_nature();
                }
                if nt.is_unknown() {
                    return bool_nature();
                }
            }
            "+" | "-" => {
                if nt.is_number() {
                    return nt;
                }
                if nt.is_unknown() {
                    return Nature::default();
                }
            }
            _ => return self.error(location, format!("unknown operator ({operator})")),
        }
        self.error(
            location,
            format!(
                "invalid operation: {operator} (mismatched type {})",
                nt.string()
            ),
        )
    }

    fn binary(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Binary {
            operator,
            left,
            right,
        } = &mut node.kind
        else {
            unreachable!("binary node")
        };
        let operator = operator.clone();
        let l = self.visit(left).deref();
        let r = self.visit(right).deref();
        let right_literal = match &right.kind {
            NodeKind::String { value } => Some(value.clone()),
            _ => None,
        };
        match operator.as_str() {
            "==" | "!=" => {
                if l.comparable_to(&r) {
                    return bool_nature();
                }
            }
            "or" | "||" | "and" | "&&" => {
                if l.is_bool() && r.is_bool() {
                    return bool_nature();
                }
                if l.maybe_compatible(&r, &[Check::Bool]) {
                    return bool_nature();
                }
            }
            "<" | ">" | ">=" | "<=" => {
                if l.is_number() && r.is_number() {
                    return bool_nature();
                }
                if l.is_string() && r.is_string() {
                    return bool_nature();
                }
                if l.is_time() && r.is_time() {
                    return bool_nature();
                }
                if l.is_duration() && r.is_duration() {
                    return bool_nature();
                }
                if l.maybe_compatible(
                    &r,
                    &[Check::Number, Check::String, Check::Time, Check::Duration],
                ) {
                    return bool_nature();
                }
            }
            "-" => {
                if l.is_number() && r.is_number() {
                    return l.promote_numeric_nature(&r);
                }
                if l.is_time() && r.is_time() {
                    return from_type(&duration_type());
                }
                if l.is_time() && r.is_duration() {
                    return from_type(&time_type());
                }
                if l.is_duration() && r.is_duration() {
                    return from_type(&duration_type());
                }
                if l.maybe_compatible(&r, &[Check::Number, Check::Time, Check::Duration]) {
                    return Nature::default();
                }
            }
            "*" => {
                if l.is_number() && r.is_number() {
                    return l.promote_numeric_nature(&r);
                }
                if l.is_number() && r.is_duration() {
                    return from_type(&duration_type());
                }
                if l.is_duration() && r.is_number() {
                    return from_type(&duration_type());
                }
                if l.is_duration() && r.is_duration() {
                    return from_type(&duration_type());
                }
                if l.maybe_compatible(&r, &[Check::Number, Check::Duration]) {
                    return Nature::default();
                }
            }
            "/" => {
                if l.is_number() && r.is_number() {
                    return float_nature();
                }
                if l.maybe_compatible(&r, &[Check::Number]) {
                    return float_nature();
                }
            }
            "**" | "^" => {
                if l.is_number() && r.is_number() {
                    return float_nature();
                }
                if l.maybe_compatible(&r, &[Check::Number]) {
                    return float_nature();
                }
            }
            "%" => {
                if l.is_integer && r.is_integer {
                    return int_nature();
                }
                if l.maybe_compatible(&r, &[Check::Integer]) {
                    return int_nature();
                }
            }
            "+" => {
                if l.is_number() && r.is_number() {
                    return l.promote_numeric_nature(&r);
                }
                if l.is_string() && r.is_string() {
                    return string_nature();
                }
                if l.is_time() && r.is_duration() {
                    return from_type(&time_type());
                }
                if l.is_duration() && r.is_time() {
                    return from_type(&time_type());
                }
                if l.is_duration() && r.is_duration() {
                    return from_type(&duration_type());
                }
                if l.maybe_compatible(
                    &r,
                    &[Check::Number, Check::String, Check::Time, Check::Duration],
                ) {
                    return Nature::default();
                }
            }
            "in" => {
                if (l.is_string() || l.is_unknown()) && r.is_struct() {
                    return bool_nature();
                }
                if r.is_map() {
                    let r_key = r.key();
                    if !l.is_unknown() && !l.assignable_to(&r_key) {
                        return self.error(
                            location,
                            format!(
                                "cannot use {} as type {} in map key",
                                l.string(),
                                r_key.string()
                            ),
                        );
                    }
                    return bool_nature();
                }
                if r.is_array() {
                    let r_elem = r.elem();
                    if !l.comparable_to(&r_elem) {
                        return self.error(
                            location,
                            format!(
                                "cannot use {} as type {} in array",
                                l.string(),
                                r_elem.string()
                            ),
                        );
                    }
                    return bool_nature();
                }
                if l.is_unknown() && r.is_any_of(&[Check::String, Check::Array, Check::Map]) {
                    return bool_nature();
                }
                if r.is_unknown() {
                    return bool_nature();
                }
            }
            "matches" => {
                if let Some(pattern) = right_literal.as_deref()
                    && let Err(message) = crate::regexp::compile(pattern)
                {
                    return self.error(location, message);
                }
                if (l.is_string() || l.is_byte_slice()) && r.is_string() {
                    return bool_nature();
                }
                if l.maybe_compatible(&r, &[Check::String]) {
                    return bool_nature();
                }
            }
            "contains" | "startsWith" | "endsWith" => {
                if l.is_string() && r.is_string() {
                    return bool_nature();
                }
                if l.maybe_compatible(&r, &[Check::String]) {
                    return bool_nature();
                }
            }
            ".." => {
                if l.is_integer && r.is_integer || l.maybe_compatible(&r, &[Check::Integer]) {
                    return from_type_array_of_int();
                }
            }
            "??" => {
                if l.nil && !r.nil {
                    return r;
                }
                if !l.nil && r.nil {
                    return l;
                }
                if l.nil && r.nil {
                    return nature::nil();
                }
                if r.assignable_to(&l) {
                    return l;
                }
                return Nature::default();
            }
            _ => return self.error(location, format!("unknown operator ({operator})")),
        }
        self.error(
            location,
            format!(
                "invalid operation: {operator} (mismatched types {} and {})",
                l.string(),
                r.string()
            ),
        )
    }

    fn member(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Member {
            node: base_node,
            property,
            optional,
            method,
        } = &mut node.kind
        else {
            unreachable!("member node")
        };
        let optional = *optional;
        let method = *method;
        if let NodeKind::Identifier { value } = &base_node.kind
            && value == "$env"
        {
            if let NodeKind::String { value: name } = &property.kind {
                let name = String::from_utf8_lossy(name).into_owned();
                let strict = if optional { false } else { self.config.strict };
                return self.ident(location, &name, strict, false);
            }
            return Nature::default();
        }
        let base = self.visit(base_node);
        let prop = self.visit(property);
        if base.is_unknown() {
            return Nature::default();
        }
        let name_of_property = match &property.kind {
            NodeKind::String { value } => Some(String::from_utf8_lossy(value).into_owned()),
            _ => None,
        };
        if let Some(name) = &name_of_property {
            if base.nil {
                return self.error(location, format!("type nil has no field {name}"));
            }
            if let Some(m) = base.method_by_name(name) {
                return m;
            }
        }
        let base = base.deref();
        match base.kind() {
            Kind::Map => {
                let mut prop = prop;
                if !prop.assignable_to(&base.key()) {
                    let prop_deref = prop.deref();
                    if prop_deref.assignable_to(&base.key()) {
                        prop = prop_deref;
                    }
                }
                if !prop.assignable_to(&base.key()) && !prop.is_unknown() {
                    return self.error(
                        property.location,
                        format!(
                            "cannot use {} to get an element from {}",
                            prop.string(),
                            base.string()
                        ),
                    );
                }
                if let Some(name) = &name_of_property
                    && let Some(map) = &base.map
                {
                    if let Some(field) = map.fields.get(name) {
                        return field.clone();
                    } else if base.strict {
                        return self.error(property.location, format!("unknown field {name}"));
                    }
                }
                base.elem()
            }
            Kind::Array | Kind::Slice => {
                let prop = prop.deref();
                if !prop.is_integer && !prop.is_unknown() {
                    return self.error(
                        property.location,
                        format!(
                            "array elements can only be selected using an integer (got {})",
                            prop.string()
                        ),
                    );
                }
                base.elem()
            }
            Kind::Struct => {
                if let Some(name) = &name_of_property {
                    if let Some(field) = base.field_by_name(name) {
                        return field;
                    }
                    if method {
                        return self.error(
                            location,
                            format!("type {} has no method {name}", base.string()),
                        );
                    }
                    return self.error(
                        location,
                        format!("type {} has no field {name}", base.string()),
                    );
                }
                self.member_error(location, &base, &name_of_property, method, &prop, property)
            }
            _ => self.member_error(location, &base, &name_of_property, method, &prop, property),
        }
    }

    fn member_error(
        &mut self,
        location: Location,
        base: &Nature,
        name: &Option<String>,
        method: bool,
        prop: &Nature,
        property: &Node,
    ) -> Nature {
        if let Some(name) = name {
            if method {
                return self.error(
                    location,
                    format!("type {} has no method {name}", base.string()),
                );
            }
            return self.error(
                location,
                format!("type {} has no field {name}", base.string()),
            );
        }
        let _ = property;
        self.error(
            location,
            format!("type {}[{}] is undefined", base.string(), prop.string()),
        )
    }

    fn slice(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Slice {
            node: base,
            from,
            to,
        } = &mut node.kind
        else {
            unreachable!("slice node")
        };
        let nt = self.visit(base);
        if nt.is_unknown() {
            return Nature::default();
        }
        match nt.kind() {
            Kind::String | Kind::Array | Kind::Slice => {}
            _ => return self.error(location, format!("cannot slice {}", nt.string())),
        }
        if let Some(from) = from {
            let from_nt = self.visit(from).deref();
            if !from_nt.is_integer && !from_nt.is_unknown() {
                return self.error(
                    from.location,
                    format!("non-integer slice index {}", from_nt.string()),
                );
            }
        }
        if let Some(to) = to {
            let to_nt = self.visit(to).deref();
            if !to_nt.is_integer && !to_nt.is_unknown() {
                return self.error(
                    to.location,
                    format!("non-integer slice index {}", to_nt.string()),
                );
            }
        }
        nt
    }

    fn call(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        // A call whose type is set already keeps its nature: the checker has
        // visited it, as Go's callNode does when it finds a type on the node.
        if let Some(ty) = &node.nature.ty
            && ty.kind() != Kind::Interface
        {
            return node.nature.clone();
        }
        let NodeKind::Call { callee, arguments } = &mut node.kind else {
            unreachable!("call node")
        };
        if let NodeKind::Identifier { value } = &callee.kind
            && value == "$env"
        {
            return self.error(location, "$env is not callable");
        }
        let nt = self.visit(callee);
        if nt.is_unknown() {
            return Nature::default();
        }
        if let Some(f) = nt.func {
            return self.check_function(f, location, arguments);
        }
        let fn_name = match &callee.kind {
            NodeKind::Identifier { value } => value.clone(),
            NodeKind::Member { property, .. } => match &property.kind {
                NodeKind::String { value } => String::from_utf8_lossy(value).into_owned(),
                _ => "function".to_owned(),
            },
            _ => "function".to_owned(),
        };
        if nt.nil {
            return self.error(
                location,
                format!("{fn_name} is nil; cannot call nil as function"),
            );
        }
        if nt.kind() == Kind::Func {
            return match self.check_arguments(&fn_name, &nt, arguments, location) {
                Ok(out) => out,
                Err(err) => {
                    if self.err.is_none() {
                        self.err = Some(err);
                    }
                    Nature::default()
                }
            };
        }
        self.error(location, format!("{} is not callable", nt.string()))
    }

    fn builtin(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Builtin {
            name, arguments, ..
        } = &mut node.kind
        else {
            unreachable!("builtin node")
        };
        let name = name.clone();
        match name.as_str() {
            "all" | "none" | "any" | "one" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(collection, Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if let Some(out) = predicate_out(&predicate) {
                    if !out.is_bool() && !out.is_unknown() {
                        return self.error(
                            arguments[1].location,
                            format!("predicate should return boolean (got {})", out.string()),
                        );
                    }
                    return bool_nature();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "filter" => {
                let mut collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(collection.clone(), Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if let Some(out) = predicate_out(&predicate) {
                    if !out.is_bool() && !out.is_unknown() {
                        return self.error(
                            arguments[1].location,
                            format!("predicate should return boolean (got {})", out.string()),
                        );
                    }
                    if collection.is_unknown() {
                        return from_type(&Ty::slice(Ty::any()));
                    }
                    collection = collection.elem();
                    return collection.make_array_of();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "map" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(
                    collection,
                    vec![VarScope {
                        name: "index".to_owned(),
                        nature: int_nature(),
                    }],
                );
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if predicate_out(&predicate).is_some() {
                    let reference = predicate.reference.as_deref().cloned().unwrap_or_default();
                    return reference.make_array_of();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "count" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                if arguments.len() == 1 {
                    return int_nature();
                }
                self.begin(collection, Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if let Some(out) = predicate_out(&predicate) {
                    if !out.is_bool() && !out.is_unknown() {
                        return self.error(
                            arguments[1].location,
                            format!("predicate should return boolean (got {})", out.string()),
                        );
                    }
                    return int_nature();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "sum" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                if arguments.len() == 2 {
                    self.begin(collection, Vec::new());
                    let predicate = self.visit(&mut arguments[1]);
                    self.end();
                    if let Some(out) = predicate_out(&predicate) {
                        return out;
                    }
                } else {
                    if collection.is_unknown() {
                        return Nature::default();
                    }
                    return collection.elem();
                }
                // Go falls out of the switch here: `sum` is then checked as the
                // function of its overloads.
                let f = builtin::lookup(&name).expect("sum is a builtin");
                self.check_function(f, location, arguments)
            }
            "find" | "findLast" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(collection.clone(), Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if let Some(out) = predicate_out(&predicate) {
                    if !out.is_bool() && !out.is_unknown() {
                        return self.error(
                            arguments[1].location,
                            format!("predicate should return boolean (got {})", out.string()),
                        );
                    }
                    if collection.is_unknown() {
                        return Nature::default();
                    }
                    return collection.elem();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "findIndex" | "findLastIndex" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(collection, Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if let Some(out) = predicate_out(&predicate) {
                    if !out.is_bool() && !out.is_unknown() {
                        return self.error(
                            arguments[1].location,
                            format!("predicate should return boolean (got {})", out.string()),
                        );
                    }
                    return int_nature();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "groupBy" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(collection.clone(), Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if predicate_out(&predicate).is_some() {
                    let grouped = collection.elem().make_array_of();
                    let mut nt = from_type(&Ty::map(Ty::any(), Ty::slice(Ty::any())));
                    nt.reference = Some(Box::new(grouped));
                    return nt;
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "sortBy" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(collection.clone(), Vec::new());
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if arguments.len() == 3 {
                    let order = self.visit(&mut arguments[2]);
                    if !order.is_string() && !order.is_unknown() {
                        return self.error(
                            arguments[2].location,
                            format!(
                                "sortBy order argument must be a string (got {})",
                                order.string()
                            ),
                        );
                    }
                }
                if predicate_out(&predicate).is_some() {
                    return collection;
                }
                self.error(
                    arguments[1].location,
                    "predicate should has one input and one output param",
                )
            }
            "reduce" => {
                let collection = self.visit(&mut arguments[0]).deref();
                if !collection.is_array() && !collection.is_unknown() {
                    return self.error(
                        arguments[0].location,
                        format!(
                            "builtin {name} takes only array (got {})",
                            collection.string()
                        ),
                    );
                }
                self.begin(
                    collection,
                    vec![
                        VarScope {
                            name: "index".to_owned(),
                            nature: int_nature(),
                        },
                        VarScope {
                            name: "acc".to_owned(),
                            nature: Nature::default(),
                        },
                    ],
                );
                let predicate = self.visit(&mut arguments[1]);
                self.end();
                if arguments.len() == 3 {
                    self.visit(&mut arguments[2]);
                }
                if predicate.is_func() && predicate.num_out() == 1 {
                    return predicate.reference.as_deref().cloned().unwrap_or_default();
                }
                self.error(
                    arguments[1].location,
                    "predicate should has two input and one output param",
                )
            }
            _ => {
                if let Some(f) = builtin::lookup(&name) {
                    if name == "get" {
                        return self.check_builtin_get(location, arguments);
                    }
                    return self.check_function(f, location, arguments);
                }
                self.error(location, format!("unknown builtin {name}"))
            }
        }
    }

    fn begin(&mut self, collection: Nature, vars: Vec<VarScope>) {
        self.predicate_scopes
            .push(PredicateScope { collection, vars });
    }

    fn end(&mut self) {
        self.predicate_scopes.pop();
    }

    fn check_builtin_get(&mut self, location: Location, arguments: &mut [Node]) -> Nature {
        if arguments.len() != 2 {
            return self.error(
                location,
                format!(
                    "invalid number of arguments (expected 2, got {})",
                    arguments.len()
                ),
            );
        }
        let base = self.visit(&mut arguments[0]);
        let prop = self.visit(&mut arguments[1]).deref();
        if let NodeKind::Identifier { value } = &arguments[0].kind
            && value == "$env"
        {
            if let NodeKind::String { value } = &arguments[1].kind {
                let name = String::from_utf8_lossy(value).into_owned();
                if let Some(nt) = self.config.env.get(&name) {
                    return nt;
                }
            }
            return Nature::default();
        }
        if base.is_unknown() {
            return Nature::default();
        }
        match base.kind() {
            Kind::Slice | Kind::Array => {
                if !prop.is_integer && !prop.is_unknown() {
                    return self.error(
                        arguments[1].location,
                        format!("non-integer slice index {}", prop.string()),
                    );
                }
                base.elem()
            }
            Kind::Map => {
                if !prop.assignable_to(&base.key()) && !prop.is_unknown() {
                    return self.error(
                        arguments[1].location,
                        format!(
                            "cannot use {} to get an element from {}",
                            prop.string(),
                            base.string()
                        ),
                    );
                }
                base.elem()
            }
            _ => self.error(
                arguments[0].location,
                format!("type {} does not support indexing", base.string()),
            ),
        }
    }

    /// Go's `checkFunction`: a builtin is checked by its validator, by its
    /// overloads, or by its function type.
    fn check_function(
        &mut self,
        f: &'static Function,
        location: Location,
        arguments: &mut [Node],
    ) -> Nature {
        if let Some(validate) = f.validate {
            let mut args = Vec::with_capacity(arguments.len());
            for arg in arguments.iter_mut() {
                let arg_nt = self.visit(arg);
                if arg_nt.is_unknown() {
                    args.push(Some(Ty::any()));
                } else {
                    args.push(arg_nt.ty.clone());
                }
            }
            return match validate(&args) {
                Ok(ty) => from_type(&ty),
                Err(message) => self.error(location, message),
            };
        }
        if f.types.is_empty() {
            let fn_nt = from_type(&f.ty());
            return match self.check_arguments(f.name, &fn_nt, arguments, location) {
                Ok(nt) => nt,
                Err(err) => {
                    if self.err.is_none() {
                        self.err = Some(err);
                    }
                    Nature::default()
                }
            };
        }
        let mut last_err = None;
        for ty in f.types {
            let fn_nt = from_type(&ty());
            match self.check_arguments(f.name, &fn_nt, arguments, location) {
                Ok(out) => return out,
                Err(err) => last_err = Some(err),
            }
        }
        if let Some(err) = last_err {
            if self.err.is_none() {
                self.err = Some(err);
            }
            return Nature::default();
        }
        self.error(location, format!("no matching overload for {}", f.name))
    }

    /// Go's `checkArguments`: the arguments against the inputs of a function.
    fn check_arguments(
        &mut self,
        name: &str,
        fn_nt: &Nature,
        arguments: &mut [Node],
        location: Location,
    ) -> Result<Nature, CheckError> {
        if fn_nt.is_unknown() {
            return Ok(Nature::default());
        }
        let num_out = fn_nt.num_out();
        if num_out == 0 {
            return Err(Error::new(
                location,
                format!("func {name} doesn't return value"),
            ));
        }
        if num_out > 2 {
            return Err(Error::new(
                location,
                format!("func {name} returns more then two values"),
            ));
        }
        let mut fn_num_in = fn_nt.num_in();
        if fn_nt.method {
            fn_num_in -= 1;
        }
        let fn_in_offset = usize::from(fn_nt.method);
        let is_variadic = fn_nt.is_variadic();
        let mut err: Option<Error> = None;
        if is_variadic {
            if arguments.len() + 1 < fn_num_in {
                err = Some(Error::new(
                    location,
                    format!("not enough arguments to call {name}"),
                ));
            }
        } else {
            if arguments.len() > fn_num_in {
                err = Some(Error::new(
                    location,
                    format!("too many arguments to call {name}"),
                ));
            }
            if arguments.len() < fn_num_in {
                err = Some(Error::new(
                    location,
                    format!("not enough arguments to call {name}"),
                ));
            }
        }
        if let Some(err) = err {
            for arg in arguments.iter_mut() {
                self.visit(arg);
            }
            return Err(err);
        }
        for (i, arg) in arguments.iter_mut().enumerate() {
            let arg_nt = self.visit(arg);
            let input = if is_variadic && i + 1 >= fn_num_in {
                fn_nt.input_elem(fn_num_in - 1 + fn_in_offset)
            } else {
                fn_nt.input(i + fn_in_offset)
            };
            if input.is_float && arg_nt.is_integer {
                replace_integers_with_floats(arg, &input);
                continue;
            }
            if input.is_integer && arg_nt.is_integer && arg_nt.kind() != input.kind() {
                replace_integers_with_integers(arg, &input);
                continue;
            }
            if arg_nt.nil {
                if input.kind() == Kind::Interface {
                    continue;
                }
                return Err(Error::new(
                    arg.location,
                    format!(
                        "cannot use nil as argument (type {}) to call {name}",
                        input.string()
                    ),
                ));
            }
            let assignable = arg_nt.assignable_to(&input);
            if !assignable && !arg_nt.is_unknown() {
                return Err(Error::new(
                    arg.location,
                    format!(
                        "cannot use {} as argument (type {}) to call {name} ",
                        arg_nt.string(),
                        input.string()
                    ),
                ));
            }
        }
        Ok(fn_nt.output(0))
    }

    fn pointer(&mut self, location: Location, name: &str) -> Nature {
        let Some(scope) = self.predicate_scopes.last() else {
            return self.error(location, "cannot use pointer accessor outside predicate");
        };
        let collection = scope.collection.clone();
        let vars = scope.vars.clone();
        if name.is_empty() {
            if collection.is_unknown() {
                return Nature::default();
            }
            if matches!(collection.kind(), Kind::Array | Kind::Slice) {
                return collection.elem();
            }
            return self.error(
                location,
                format!("cannot use {{{}}} as array", collection.string()),
            );
        }
        for var in &vars {
            if var.name == name {
                return var.nature.clone();
            }
        }
        self.error(location, format!("unknown pointer #{name}"))
    }

    fn conditional(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Conditional {
            cond, exp1, exp2, ..
        } = &mut node.kind
        else {
            unreachable!("conditional node")
        };
        let c = self.visit(cond).deref();
        if !c.is_bool() && !c.is_unknown() {
            return self.error(
                cond.location,
                format!(
                    "non-bool expression (type {}) used as condition",
                    c.string()
                ),
            );
        }
        let t1 = self.visit(exp1);
        let t2 = self.visit(exp2);
        let _ = location;
        if t1.nil && !t2.nil {
            return t2;
        }
        if !t1.nil && t2.nil {
            return t1;
        }
        if t1.nil && t2.nil {
            return nature::nil();
        }
        if t1.assignable_to(&t2) {
            if t1.is_array() && t2.is_array() {
                let e1 = t1.elem();
                let e2 = t2.elem();
                if !e1.assignable_to(&e2) || !e2.assignable_to(&e1) {
                    return from_type(&Ty::slice(Ty::any()));
                }
            }
            return t1;
        }
        Nature::default()
    }

    fn variable_declarator(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::VariableDeclarator { name, value, expr } = &mut node.kind else {
            unreachable!("variable declarator")
        };
        let name = name.clone();
        if self.config.env.get(&name).is_some() {
            return self.error(location, format!("cannot redeclare {name}"));
        }
        if builtin::lookup(&name).is_some() && !self.config.disabled.contains(&name) {
            return self.error(location, format!("cannot redeclare builtin {name}"));
        }
        for var in self.var_scopes.iter().rev() {
            if var.name == name {
                return self.error(location, format!("cannot redeclare variable {name}"));
            }
        }
        let var_nature = self.visit(value);
        self.var_scopes.push(VarScope {
            name,
            nature: var_nature,
        });
        let expr_nature = self.visit(expr);
        self.var_scopes.pop();
        expr_nature
    }

    fn sequence(&mut self, node: &mut Node) -> Nature {
        let location = node.location;
        let NodeKind::Sequence { nodes } = &mut node.kind else {
            unreachable!("sequence node")
        };
        if nodes.is_empty() {
            return self.error(location, "empty sequence expression");
        }
        let mut last = Nature::default();
        for n in nodes.iter_mut() {
            last = self.visit(n);
        }
        last
    }

    fn array(&mut self, node: &mut Node) -> Nature {
        let NodeKind::Array { nodes } = &mut node.kind else {
            unreachable!("array node")
        };
        let mut prev = Nature::default();
        let mut all_elements_are_same_type = true;
        for (i, n) in nodes.iter_mut().enumerate() {
            let curr = self.visit(n);
            if i > 0 && curr.kind() != prev.kind() {
                all_elements_are_same_type = false;
            }
            prev = curr;
        }
        if all_elements_are_same_type {
            return prev.make_array_of();
        }
        from_type(&Ty::slice(Ty::any()))
    }
}

/// The output of a predicate function, when the predicate takes one input
/// and gives one output: the check the builtins make before using it.
fn predicate_out(predicate: &Nature) -> Option<Nature> {
    if predicate.is_func()
        && predicate.num_out() == 1
        && predicate.num_in() == 1
        && predicate.is_first_arg_unknown()
    {
        return Some(predicate.output(0));
    }
    None
}

fn from_type_array_of_int() -> Nature {
    nature::array_from_type(&Ty::int())
}

/// Go's `traverseAndReplaceIntegerNodesWithFloatNodes`: integer literals in
/// an arithmetic expression become floats when the input is a float.
fn replace_integers_with_floats(node: &mut Node, new_nature: &Nature) {
    match &mut node.kind {
        NodeKind::Integer { value } => {
            let value = *value as f64;
            node.kind = NodeKind::Float { value };
            node.nature = new_nature.clone();
        }
        NodeKind::Unary { node: inner, .. } => replace_integers_with_floats(inner, new_nature),
        NodeKind::Binary {
            operator,
            left,
            right,
        } => {
            if matches!(operator.as_str(), "+" | "-" | "*") {
                replace_integers_with_floats(left, new_nature);
                replace_integers_with_floats(right, new_nature);
            }
        }
        _ => {}
    }
}

/// Go's `traverseAndReplaceIntegerNodesWithIntegerNodes`: the literals take
/// the type of the input.
fn replace_integers_with_integers(node: &mut Node, new_nature: &Nature) {
    match &mut node.kind {
        NodeKind::Integer { .. } => {
            node.nature.ty = new_nature.ty.clone();
        }
        NodeKind::Unary { node: inner, .. } => {
            node.nature.ty = new_nature.ty.clone();
            replace_integers_with_integers(inner, new_nature);
        }
        NodeKind::Binary {
            operator,
            left,
            right,
        } => {
            if matches!(operator.as_str(), "+" | "-" | "*") {
                replace_integers_with_integers(left, new_nature);
                replace_integers_with_integers(right, new_nature);
            }
        }
        _ => {}
    }
}
