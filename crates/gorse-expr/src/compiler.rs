// Ports: github.com/expr-lang/expr/compiler/compiler.go @ v1.17.8

//! The compiler: it turns a checked tree into the bytecode the VM runs.
//!
//! Each instruction records the location of the node it was emitted for, and
//! a failing instruction reports that location, as in Go. The instructions
//! are one enum with their operands, not Go's opcode and argument pair, and
//! the typed fast paths of Go (`OpEqualInt`, `OpCallTyped`, ...) are not
//! emitted: the generic instruction gives the same results (WP-3-6 notes).
//! Pointer dereferences are not emitted, as pointers are not modelled.

use crate::ast::{Node, NodeKind};
use crate::builtin::{self, Function};
use crate::checker::{field_index, method_index};
use crate::conf::Config;
use crate::file::{Location, Source};
use crate::nature::Nature;
use crate::ty::Ty;
use crate::value::Value;
use crate::vm::{Constant, Field, Instr, MethodRef, Op, Program};

/// Compiles a checked tree; `optimize` runs first when the config asks.
pub fn compile(
    node: &Node,
    source: &Source,
    config: &Config,
) -> Result<Program, crate::file::Error> {
    let mut compiler = Compiler {
        config,
        instrs: Vec::new(),
        variables: 0,
        scopes: Vec::new(),
        constants: Vec::new(),
        functions: Vec::new(),
        nodes: Vec::new(),
        chains: Vec::new(),
    };
    compiler.compile(node);
    if config.optimize {
        compiler.optimize();
    }
    Ok(Program {
        source: source.clone(),
        root: root_nature(node),
        instrs: compiler.instrs,
        variables: compiler.variables,
        constants: compiler.constants,
        functions: compiler.functions,
    })
}

/// A node being compiled: its location for the instructions it emits, and
/// whether it is the left-hand side of a `??`, which decides how a chain ends.
struct Frame {
    location: Location,
    is_coalesce: bool,
}

struct Compiler<'a> {
    config: &'a Config,
    instrs: Vec<Instr>,
    variables: usize,
    scopes: Vec<(String, usize)>,
    constants: Vec<Constant>,
    functions: Vec<&'static Function>,
    nodes: Vec<Frame>,
    chains: Vec<Vec<usize>>,
}

/// Go's `placeholder`: the operand of a jump that is patched later.
const PLACEHOLDER: i64 = 12345;

impl Compiler<'_> {
    fn emit(&mut self, op: Op) -> usize {
        let location = self.nodes.last().map(|f| f.location).unwrap_or_default();
        self.emit_at(location, op)
    }

    fn emit_at(&mut self, location: Location, op: Op) -> usize {
        self.instrs.push(Instr { op, location });
        self.instrs.len()
    }

    fn emit_push(&mut self, value: Value) -> usize {
        let index = self.add_constant(Constant::Value(value));
        self.emit(Op::Push(index))
    }

    fn add_constant(&mut self, constant: Constant) -> usize {
        self.constants.push(constant);
        self.constants.len() - 1
    }

    fn add_function(&mut self, f: &'static Function) -> usize {
        if let Some(index) = self.functions.iter().position(|g| g.name == f.name) {
            return index;
        }
        self.functions.push(f);
        self.functions.len() - 1
    }

    fn patch_jump(&mut self, placeholder: usize) {
        let offset = self.instrs.len() as i64 - placeholder as i64;
        let instr = &mut self.instrs[placeholder - 1];
        instr.op = match instr.op {
            Op::Jump(_) => Op::Jump(offset),
            Op::JumpIfTrue(_) => Op::JumpIfTrue(offset),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(offset),
            Op::JumpIfNil(_) => Op::JumpIfNil(offset),
            Op::JumpIfNotNil(_) => Op::JumpIfNotNil(offset),
            Op::JumpIfEnd(_) => Op::JumpIfEnd(offset),
            other => other,
        };
    }

    fn calc_backward_jump(&self, to: usize) -> i64 {
        self.instrs.len() as i64 + 1 - to as i64
    }

    fn add_variable(&mut self) -> usize {
        self.variables += 1;
        self.variables - 1
    }

    fn compile(&mut self, node: &Node) {
        let is_coalesce =
            matches!(&node.kind, NodeKind::Binary { operator, .. } if operator == "??");
        self.nodes.push(Frame {
            location: node.location,
            is_coalesce,
        });
        match &node.kind {
            NodeKind::Nil => {
                self.emit(Op::Nil);
            }
            NodeKind::Identifier { value } => self.identifier(node, value),
            NodeKind::Integer { value } => {
                self.emit_push(Value::Int(*value));
            }
            NodeKind::Float { value } => {
                self.emit_push(Value::Float(*value));
            }
            NodeKind::Bool { value } => {
                self.emit(if *value { Op::True } else { Op::False });
            }
            NodeKind::String { value } => {
                self.emit_push(Value::Str(value.as_slice().into()));
            }
            NodeKind::Bytes { value } => {
                self.emit_push(Value::Bytes(value.as_slice().into()));
            }
            NodeKind::Constant { value } => {
                if value.is_nil() {
                    self.emit(Op::Nil);
                } else {
                    self.emit_push(value.clone());
                }
            }
            NodeKind::Unary {
                operator,
                node: inner,
            } => {
                self.compile(inner);
                match operator.as_str() {
                    "!" | "not" => {
                        self.emit(Op::Not);
                    }
                    "+" => {}
                    "-" => {
                        self.emit(Op::Negate);
                    }
                    _ => panic!("unknown operator ({operator})"),
                }
            }
            NodeKind::Binary {
                operator,
                left,
                right,
            } => self.binary(operator, left, right),
            NodeKind::Chain { node: inner } => self.chain(node, inner),
            NodeKind::Member { .. } => self.member(node),
            NodeKind::Slice {
                node: base,
                from,
                to,
            } => {
                self.compile(base);
                match to {
                    Some(to) => self.compile(to),
                    None => {
                        self.emit(Op::Len);
                    }
                }
                match from {
                    Some(from) => self.compile(from),
                    None => {
                        self.emit_push(Value::Int(0));
                    }
                }
                self.emit(Op::Slice);
            }
            NodeKind::Call { callee, arguments } => self.call(callee, arguments),
            NodeKind::Builtin { .. } => self.builtin(node),
            NodeKind::Predicate { node: inner } => self.compile(inner),
            NodeKind::Pointer { name } => match name.as_str() {
                "index" => {
                    self.emit(Op::GetIndex);
                }
                "acc" => {
                    self.emit(Op::GetAcc);
                }
                "" => {
                    self.emit(Op::Pointer);
                }
                _ => panic!("unknown pointer {name}"),
            },
            NodeKind::VariableDeclarator { name, value, expr } => {
                self.compile(value);
                let index = self.add_variable();
                self.emit(Op::Store(index));
                self.scopes.push((name.clone(), index));
                self.compile(expr);
                self.scopes.pop();
            }
            NodeKind::Sequence { nodes } => {
                for (i, n) in nodes.iter().enumerate() {
                    self.compile(n);
                    if i + 1 < nodes.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            NodeKind::Conditional {
                cond, exp1, exp2, ..
            } => {
                self.compile(cond);
                let otherwise = self.emit(Op::JumpIfFalse(PLACEHOLDER));
                self.emit(Op::Pop);
                self.compile(exp1);
                let end = self.emit(Op::Jump(PLACEHOLDER));
                self.patch_jump(otherwise);
                self.emit(Op::Pop);
                self.compile(exp2);
                self.patch_jump(end);
            }
            NodeKind::Array { nodes } => {
                for n in nodes {
                    self.compile(n);
                }
                self.emit_push(Value::Int(nodes.len() as i64));
                self.emit(Op::Array);
            }
            NodeKind::Map { pairs } => {
                for pair in pairs {
                    self.compile(pair);
                }
                self.emit_push(Value::Int(pairs.len() as i64));
                self.emit(Op::Map);
            }
            NodeKind::Pair { key, value } => {
                self.compile(key);
                self.compile(value);
            }
        }
        self.nodes.pop();
    }

    fn lookup_variable(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find(|(scope, _)| scope == name)
            .map(|(_, index)| *index)
    }

    fn identifier(&mut self, node: &Node, name: &str) {
        if let Some(index) = self.lookup_variable(name) {
            self.emit(Op::LoadVar(index));
            return;
        }
        if name == "$env" {
            self.emit(Op::LoadEnv);
            return;
        }
        let env = &self.config.env;
        if env.is_fast_map() {
            let index = self.add_constant(Constant::Name(name.to_owned()));
            self.emit(Op::LoadFast(index));
        } else if let Some((index, _)) = field_index(env, node) {
            let index = self.add_constant(Constant::Field(Field {
                index,
                path: vec![name.to_owned()],
            }));
            self.emit(Op::LoadField(index));
        } else if let Some((index, _)) = method_index(env, node) {
            let index = self.add_constant(Constant::Method(MethodRef {
                name: name.to_owned(),
                index,
            }));
            self.emit(Op::LoadMethod(index));
        } else {
            let index = self.add_constant(Constant::Name(name.to_owned()));
            self.emit(Op::LoadConst(index));
        }
    }

    fn binary(&mut self, operator: &str, left: &Node, right: &Node) {
        match operator {
            "==" => {
                self.compile_pair(left, right);
                self.emit(Op::Equal);
            }
            "!=" => {
                self.compile_pair(left, right);
                self.emit(Op::Equal);
                self.emit(Op::Not);
            }
            "or" | "||" => {
                if !self.config.short_circuit {
                    self.compile_pair(left, right);
                    self.emit(Op::Or);
                    return;
                }
                self.compile(left);
                let end = self.emit(Op::JumpIfTrue(PLACEHOLDER));
                self.emit(Op::Pop);
                self.compile(right);
                self.patch_jump(end);
            }
            "and" | "&&" => {
                if !self.config.short_circuit {
                    self.compile_pair(left, right);
                    self.emit(Op::And);
                    return;
                }
                self.compile(left);
                let end = self.emit(Op::JumpIfFalse(PLACEHOLDER));
                self.emit(Op::Pop);
                self.compile(right);
                self.patch_jump(end);
            }
            "??" => {
                self.compile(left);
                let end = self.emit(Op::JumpIfNotNil(PLACEHOLDER));
                self.emit(Op::Pop);
                self.compile(right);
                self.patch_jump(end);
            }
            _ => {
                self.compile_pair(left, right);
                let op = match operator {
                    "<" => Op::Less,
                    ">" => Op::More,
                    "<=" => Op::LessOrEqual,
                    ">=" => Op::MoreOrEqual,
                    "+" => Op::Add,
                    "-" => Op::Subtract,
                    "*" => Op::Multiply,
                    "/" => Op::Divide,
                    "%" => Op::Modulo,
                    "**" | "^" => Op::Exponent,
                    "in" => Op::In,
                    "matches" => {
                        return self.matches(left, right);
                    }
                    "contains" => Op::Contains,
                    "startsWith" => Op::StartsWith,
                    "endsWith" => Op::EndsWith,
                    ".." => Op::Range,
                    _ => panic!("unknown operator ({operator})"),
                };
                self.emit(op);
            }
        }
    }

    fn compile_pair(&mut self, left: &Node, right: &Node) {
        self.compile(left);
        self.compile(right);
    }

    fn matches(&mut self, left: &Node, right: &Node) {
        if let NodeKind::String { value } = &right.kind {
            let pattern = String::from_utf8_lossy(value).into_owned();
            let regex = match crate::regexp::compile(&pattern) {
                Ok(regex) => regex,
                Err(message) => panic!("{message}"),
            };
            self.compile(left);
            let index = self.add_constant(Constant::Regex(regex));
            self.emit(Op::MatchesConst(index));
        } else {
            self.compile_pair(left, right);
            self.emit(Op::Matches);
        }
    }

    fn chain(&mut self, node: &Node, inner: &Node) {
        let _ = node;
        self.chains.push(Vec::new());
        self.compile(inner);
        let placeholders = self.chains.last().cloned().unwrap_or_default();
        for ph in placeholders {
            self.patch_jump(ph);
        }
        let parent_is_coalesce =
            self.nodes.len() >= 2 && self.nodes[self.nodes.len() - 2].is_coalesce;
        if !parent_is_coalesce {
            let j = self.emit(Op::JumpIfNotNil(PLACEHOLDER));
            self.emit(Op::Pop);
            self.emit(Op::Nil);
            self.patch_jump(j);
        }
        self.chains.pop();
    }

    fn member(&mut self, node: &Node) {
        let NodeKind::Member {
            node: base_node,
            property,
            optional,
            ..
        } = &node.kind
        else {
            unreachable!("member node")
        };
        let env = &self.config.env;
        if let Some((index, name)) = method_index(env, node) {
            self.compile(base_node);
            let constant = self.add_constant(Constant::Method(MethodRef { name, index }));
            self.emit(Op::Method(constant));
            return;
        }

        // Go loads a chain of struct fields (`a.b.c`) with one instruction
        // when every link is a field. The walk follows the links down to
        // the first one that is not, or to an optional link, and the node
        // that ends the walk gives the location and the optionality.
        let mut base: &Node = base_node;
        let mut location = node.location;
        let mut optional = *optional;
        let mut op_is_field = false;
        let mut index = Vec::new();
        let mut path = Vec::new();
        if let Some((field, name)) = field_index(env, node) {
            op_is_field = true;
            index = field;
            path = vec![name];
            while !optional {
                if let NodeKind::Identifier { .. } = &base.kind
                    && let Some((ident_index, ident_name)) = field_index(env, base) {
                        let mut full = ident_index;
                        full.extend(index.iter().copied());
                        path.insert(0, ident_name);
                        let constant = self.add_constant(Constant::Field(Field {
                            index: full,
                            path: path.clone(),
                        }));
                        self.emit_at(base.location, Op::LoadField(constant));
                        return;
                    }
                let NodeKind::Member {
                    node: member_base,
                    optional: member_optional,
                    ..
                } = &base.kind
                else {
                    break;
                };
                let Some((member_index, member_name)) = field_index(env, base) else {
                    break;
                };
                let mut full = member_index;
                full.extend(index.iter().copied());
                index = full;
                path.insert(0, member_name);
                location = base.location;
                optional = *member_optional;
                base = member_base;
            }
        }
        self.compile(base);
        if optional && !self.chains.is_empty() {
            let ph = self.emit(Op::JumpIfNil(PLACEHOLDER));
            self.chains.last_mut().expect("chain").push(ph);
        }
        if op_is_field {
            let constant = self.add_constant(Constant::Field(Field { index, path }));
            self.emit_at(location, Op::FetchField(constant));
            return;
        }
        self.compile(property);
        self.emit(Op::Fetch);
    }

    fn call(&mut self, callee: &Node, arguments: &[Node]) {
        for arg in arguments {
            self.compile(arg);
        }
        self.compile(callee);
        self.emit(Op::Call(arguments.len()));
    }

    fn builtin(&mut self, node: &Node) {
        let NodeKind::Builtin {
            name,
            arguments,
            threshold,
            map,
            throws,
        } = &node.kind
        else {
            unreachable!("builtin node")
        };
        match name.as_str() {
            "all" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let loop_break = self.emit_loop_with_break(|c| {
                    c.compile(&arguments[1]);
                    let brk = c.emit(Op::JumpIfFalse(PLACEHOLDER));
                    c.emit(Op::Pop);
                    brk
                });
                self.emit(Op::True);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "none" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let loop_break = self.emit_loop_with_break(|c| {
                    c.compile(&arguments[1]);
                    c.emit(Op::Not);
                    let brk = c.emit(Op::JumpIfFalse(PLACEHOLDER));
                    c.emit(Op::Pop);
                    brk
                });
                self.emit(Op::True);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "any" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let loop_break = self.emit_loop_with_break(|c| {
                    c.compile(&arguments[1]);
                    let brk = c.emit(Op::JumpIfTrue(PLACEHOLDER));
                    c.emit(Op::Pop);
                    brk
                });
                self.emit(Op::False);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "one" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    c.emit_cond(|c| {
                        c.emit(Op::IncrementCount);
                    });
                });
                self.emit(Op::GetCount);
                self.emit_push(Value::Int(1));
                self.emit(Op::Equal);
                self.emit(Op::End);
            }
            "filter" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    c.emit_cond(|c| {
                        c.emit(Op::IncrementCount);
                        match map {
                            Some(map) => c.compile(map),
                            None => {
                                c.emit(Op::Pointer);
                            }
                        }
                    });
                });
                self.emit(Op::GetCount);
                self.emit(Op::End);
                self.emit(Op::Array);
            }
            "map" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                });
                self.emit(Op::GetLen);
                self.emit(Op::End);
                self.emit(Op::Array);
            }
            "count" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let mut loop_break = 0;
                let threshold = *threshold;
                self.emit_loop(|c| {
                    if arguments.len() == 2 {
                        c.compile(&arguments[1]);
                    } else {
                        c.emit(Op::Pointer);
                    }
                    c.emit_cond(|c| {
                        c.emit(Op::IncrementCount);
                        if let Some(threshold) = threshold {
                            c.emit(Op::GetCount);
                            c.emit(Op::Int(threshold));
                            c.emit(Op::MoreOrEqual);
                            loop_break = c.emit(Op::JumpIfTrue(PLACEHOLDER));
                            c.emit(Op::Pop);
                        }
                    });
                });
                self.emit(Op::GetCount);
                if let Some(_threshold) = threshold {
                    let end = self.emit(Op::Jump(PLACEHOLDER));
                    self.patch_jump(loop_break);
                    self.emit(Op::Pop);
                    self.emit(Op::GetCount);
                    self.patch_jump(end);
                }
                self.emit(Op::End);
            }
            "sum" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                self.emit(Op::Int(0));
                self.emit(Op::SetAcc);
                self.emit_loop(|c| {
                    if arguments.len() == 2 {
                        c.compile(&arguments[1]);
                    } else {
                        c.emit(Op::Pointer);
                    }
                    c.emit(Op::GetAcc);
                    c.emit(Op::Add);
                    c.emit(Op::SetAcc);
                });
                self.emit(Op::GetAcc);
                self.emit(Op::End);
            }
            "find" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let mut loop_break = 0;
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    let noop = c.emit(Op::JumpIfFalse(PLACEHOLDER));
                    c.emit(Op::Pop);
                    match map {
                        Some(map) => c.compile(map),
                        None => {
                            c.emit(Op::Pointer);
                        }
                    }
                    loop_break = c.emit(Op::Jump(PLACEHOLDER));
                    c.patch_jump(noop);
                    c.emit(Op::Pop);
                });
                self.emit_nil_or_throw(*throws);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "findIndex" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let mut loop_break = 0;
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    let noop = c.emit(Op::JumpIfFalse(PLACEHOLDER));
                    c.emit(Op::Pop);
                    c.emit(Op::GetIndex);
                    loop_break = c.emit(Op::Jump(PLACEHOLDER));
                    c.patch_jump(noop);
                    c.emit(Op::Pop);
                });
                self.emit(Op::Nil);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "findLast" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let mut loop_break = 0;
                self.emit_loop_backwards(|c| {
                    c.compile(&arguments[1]);
                    let noop = c.emit(Op::JumpIfFalse(PLACEHOLDER));
                    c.emit(Op::Pop);
                    match map {
                        Some(map) => c.compile(map),
                        None => {
                            c.emit(Op::Pointer);
                        }
                    }
                    loop_break = c.emit(Op::Jump(PLACEHOLDER));
                    c.patch_jump(noop);
                    c.emit(Op::Pop);
                });
                self.emit_nil_or_throw(*throws);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "findLastIndex" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                let mut loop_break = 0;
                self.emit_loop_backwards(|c| {
                    c.compile(&arguments[1]);
                    let noop = c.emit(Op::JumpIfFalse(PLACEHOLDER));
                    c.emit(Op::Pop);
                    c.emit(Op::GetIndex);
                    loop_break = c.emit(Op::Jump(PLACEHOLDER));
                    c.patch_jump(noop);
                    c.emit(Op::Pop);
                });
                self.emit(Op::Nil);
                self.patch_jump(loop_break);
                self.emit(Op::End);
            }
            "groupBy" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                self.emit(Op::Create(1));
                self.emit(Op::SetAcc);
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    c.emit(Op::GroupBy);
                });
                self.emit(Op::GetAcc);
                self.emit(Op::End);
            }
            "sortBy" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                if arguments.len() == 3 {
                    self.compile(&arguments[2]);
                } else {
                    self.emit_push(Value::str("asc"));
                }
                self.emit(Op::Create(2));
                self.emit(Op::SetAcc);
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    c.emit(Op::SortBy);
                });
                self.emit(Op::Sort);
                self.emit(Op::End);
            }
            "reduce" => {
                self.compile(&arguments[0]);
                self.emit(Op::Begin);
                if arguments.len() == 3 {
                    self.compile(&arguments[2]);
                    self.emit(Op::SetAcc);
                } else {
                    let empty = self.emit(Op::JumpIfEnd(PLACEHOLDER));
                    self.emit(Op::Pointer);
                    self.emit(Op::IncrementIndex);
                    self.emit(Op::SetAcc);
                    let jump_past_error = self.emit(Op::Jump(PLACEHOLDER));
                    self.patch_jump(empty);
                    let index = self.add_constant(Constant::Error(
                        "reduce of empty array with no initial value".to_owned(),
                    ));
                    self.emit(Op::Push(index));
                    self.emit(Op::Throw);
                    self.patch_jump(jump_past_error);
                }
                self.emit_loop(|c| {
                    c.compile(&arguments[1]);
                    c.emit(Op::SetAcc);
                });
                self.emit(Op::GetAcc);
                self.emit(Op::End);
            }
            _ => {
                let Some(f) = builtin::lookup(name) else {
                    panic!("unknown builtin {name}");
                };
                for arg in arguments {
                    self.compile(arg);
                }
                let index = self.add_function(f);
                if f.safe.is_some() {
                    self.emit(Op::CallSafe(index, arguments.len()));
                } else {
                    self.emit(Op::CallFunc(index, arguments.len()));
                }
            }
        }
    }

    /// The value a `find` loop leaves when nothing matched: nil, or the error
    /// `reflect: slice index out of range` when the node throws.
    fn emit_nil_or_throw(&mut self, throws: bool) {
        if throws {
            let index = self.add_constant(Constant::Error(
                "reflect: slice index out of range".to_owned(),
            ));
            self.emit(Op::Push(index));
            self.emit(Op::Throw);
        } else {
            self.emit(Op::Nil);
        }
    }

    fn emit_cond(&mut self, body: impl FnOnce(&mut Self)) {
        let noop = self.emit(Op::JumpIfFalse(PLACEHOLDER));
        self.emit(Op::Pop);
        body(self);
        let jmp = self.emit(Op::Jump(PLACEHOLDER));
        self.patch_jump(noop);
        self.emit(Op::Pop);
        self.patch_jump(jmp);
    }

    fn emit_loop(&mut self, body: impl FnOnce(&mut Self)) {
        let begin = self.instrs.len();
        let end = self.emit(Op::JumpIfEnd(PLACEHOLDER));
        body(self);
        self.emit(Op::IncrementIndex);
        let back = self.calc_backward_jump(begin);
        self.emit(Op::JumpBackward(back));
        self.patch_jump(end);
    }

    /// A loop whose body returns the placeholder of its break jump; the
    /// caller patches it to the end of the loop.
    fn emit_loop_with_break(&mut self, body: impl FnOnce(&mut Self) -> usize) -> usize {
        let begin = self.instrs.len();
        let end = self.emit(Op::JumpIfEnd(PLACEHOLDER));
        let loop_break = body(self);
        self.emit(Op::IncrementIndex);
        let back = self.calc_backward_jump(begin);
        self.emit(Op::JumpBackward(back));
        self.patch_jump(end);
        loop_break
    }

    fn emit_loop_backwards(&mut self, body: impl FnOnce(&mut Self)) {
        self.emit(Op::GetLen);
        self.emit(Op::Int(1));
        self.emit(Op::Subtract);
        self.emit(Op::SetIndex);
        let begin = self.instrs.len();
        self.emit(Op::GetIndex);
        self.emit(Op::Int(0));
        self.emit(Op::MoreOrEqual);
        let end = self.emit(Op::JumpIfFalse(PLACEHOLDER));
        body(self);
        self.emit(Op::DecrementIndex);
        let back = self.calc_backward_jump(begin);
        self.emit(Op::JumpBackward(back));
        self.patch_jump(end);
    }

    /// Go's `optimize`: a jump to another jump of the same kind jumps to its
    /// target directly.
    fn optimize(&mut self) {
        for i in 0..self.instrs.len() {
            let offset = match self.instrs[i].op {
                Op::JumpIfTrue(o) | Op::JumpIfFalse(o) | Op::JumpIfNil(o) | Op::JumpIfNotNil(o) => {
                    o
                }
                _ => continue,
            };
            let mut target = (i as i64 + offset + 1) as usize;
            while target < self.instrs.len()
                && same_jump(&self.instrs[target].op, &self.instrs[i].op)
            {
                target = (target as i64 + jump_offset(&self.instrs[target].op) + 1) as usize;
            }
            let new_offset = target as i64 - i as i64 - 1;
            self.instrs[i].op = with_jump_offset(self.instrs[i].op, new_offset);
        }
    }
}

fn same_jump(a: &Op, b: &Op) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn jump_offset(op: &Op) -> i64 {
    match op {
        Op::JumpIfTrue(o) | Op::JumpIfFalse(o) | Op::JumpIfNil(o) | Op::JumpIfNotNil(o) => *o,
        _ => 0,
    }
}

fn with_jump_offset(op: Op, offset: i64) -> Op {
    match op {
        Op::JumpIfTrue(_) => Op::JumpIfTrue(offset),
        Op::JumpIfFalse(_) => Op::JumpIfFalse(offset),
        Op::JumpIfNil(_) => Op::JumpIfNil(offset),
        Op::JumpIfNotNil(_) => Op::JumpIfNotNil(offset),
        other => other,
    }
}

/// The nature of the root, which Go's `Node().Type()` reports as `interface {}`
/// when the checker knew no type for it.
fn root_nature(node: &Node) -> Nature {
    let mut root = node.nature.clone();
    if root.ty.is_none() {
        root.ty = Some(Ty::any());
    }
    root
}
