// Ports: github.com/expr-lang/expr/vm/{vm,program,utils,opcodes}.go @ v1.17.8
// (the generic instructions; the typed fast paths are not ported)

//! The virtual machine: it runs a compiled [`Program`] on an environment.
//!
//! A panic in Go's VM is an error here, and it reports the location of the
//! instruction that failed, with the panic's message. The memory budget
//! counts array and range sizes, as Go's `memGrow` does.

use std::cell::RefCell;
use std::sync::Arc;

use gorse_common::gosort;
use regex::bytes::Regex;

use crate::builtin::Function;
use crate::file::{self, Location, Source};
use crate::nature::Nature;
use crate::ty::Ty;
use crate::value::{self, Panic, Value};

/// `conf.DefaultMemoryBudget`.
pub const DEFAULT_MEMORY_BUDGET: u64 = 1_000_000;

/// A compiled expression.
pub struct Program {
    pub(crate) source: Source,
    pub(crate) root: Nature,
    pub(crate) instrs: Vec<Instr>,
    pub(crate) variables: usize,
    pub(crate) constants: Vec<Constant>,
    pub(crate) functions: Vec<&'static Function>,
}

impl Program {
    /// The nature of the root node: its type and kind, as Go's `Node().Type()`.
    pub fn root(&self) -> &Nature {
        &self.root
    }

    /// The type of the root node, when it is known.
    pub fn root_type(&self) -> Option<&Ty> {
        self.root.ty.as_ref()
    }
}

/// An instruction and the location of the node it was made for.
#[derive(Debug, Clone)]
pub struct Instr {
    /// What to do.
    pub op: Op,
    /// Where in the source the node is, for error messages.
    pub location: Location,
}

/// A field of a struct to load: its index path and the names along it.
#[derive(Debug, Clone)]
pub struct Field {
    /// The index path through the struct's fields.
    pub index: Vec<usize>,
    /// The names of the fields along the path.
    pub path: Vec<String>,
}

/// A method to load: its name and its index in the receiver's type.
#[derive(Debug, Clone)]
pub struct MethodRef {
    /// The method's name.
    pub name: String,
    /// The index of the method.
    pub index: usize,
}

/// A value the program refers to by index.
#[derive(Clone)]
pub enum Constant {
    /// A value to push.
    Value(Value),
    /// A name to look up in the environment.
    Name(String),
    /// A field to load.
    Field(Field),
    /// A method to load.
    Method(MethodRef),
    /// A regular expression to match with, compiled once.
    Regex(Regex),
    /// An error to throw.
    Error(String),
}

/// What an instruction does. Operands are indexes into the constants and
/// variables, or jump offsets from the next instruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Push a constant.
    Push(usize),
    /// Push an `int`.
    Int(i64),
    /// Pop the top value.
    Pop,
    /// Pop into a variable.
    Store(usize),
    /// Push a variable.
    LoadVar(usize),
    /// Push a name from the environment, looked up with `Fetch`.
    LoadConst(usize),
    /// Push a field of the environment.
    LoadField(usize),
    /// Push a name from an environment map.
    LoadFast(usize),
    /// Push a method of the environment.
    LoadMethod(usize),
    /// Push the environment.
    LoadEnv,
    /// Pop a key and a value, push the entry.
    Fetch,
    /// Pop a value, push its field.
    FetchField(usize),
    /// Pop a value, push its method.
    Method(usize),
    /// Push `true`.
    True,
    /// Push `false`.
    False,
    /// Push `nil`.
    Nil,
    /// Pop a number, push its negation.
    Negate,
    /// Pop a bool, push its negation.
    Not,
    /// Pop two values, push whether they are equal.
    Equal,
    /// Jump forward.
    Jump(i64),
    /// Jump forward when the top is true.
    JumpIfTrue(i64),
    /// Jump forward when the top is false.
    JumpIfFalse(i64),
    /// Jump forward when the top is nil.
    JumpIfNil(i64),
    /// Jump forward when the top is not nil.
    JumpIfNotNil(i64),
    /// Jump forward when the loop of the current scope is at its end.
    JumpIfEnd(i64),
    /// Jump backward.
    JumpBackward(i64),
    /// Pop two values, push `needle in array`.
    In,
    /// Pop two values, push `a < b`.
    Less,
    /// Pop two values, push `a > b`.
    More,
    /// Pop two values, push `a <= b`.
    LessOrEqual,
    /// Pop two values, push `a >= b`.
    MoreOrEqual,
    /// Pop two values, push their sum.
    Add,
    /// Pop two values, push their difference.
    Subtract,
    /// Pop two values, push their product.
    Multiply,
    /// Pop two values, push their quotient.
    Divide,
    /// Pop two values, push their remainder.
    Modulo,
    /// Pop two values, push the first to the power of the second.
    Exponent,
    /// Pop two values, push the range between them.
    Range,
    /// Pop a pattern and a string, push whether it matches.
    Matches,
    /// Pop a string, push whether it matches the constant regular expression.
    MatchesConst(usize),
    /// Pop two strings, push whether the first contains the second.
    Contains,
    /// Pop two strings, push whether the first starts with the second.
    StartsWith,
    /// Pop two strings, push whether the first ends with the second.
    EndsWith,
    /// Pop an array and its bounds, push the slice.
    Slice,
    /// Pop a function and its arguments, push the result.
    Call(usize),
    /// Call a builtin with its arguments.
    CallFunc(usize, usize),
    /// Call a builtin that reports the memory it uses.
    CallSafe(usize, usize),
    /// Pop the length and the elements, push an array.
    Array,
    /// Pop the length and the entries, push a map.
    Map,
    /// Push the length of the top value, leaving it in place.
    Len,
    /// Increase the index of the loop.
    IncrementIndex,
    /// Decrease the index of the loop.
    DecrementIndex,
    /// Increase the count of the loop.
    IncrementCount,
    /// Push the index of the loop.
    GetIndex,
    /// Push the count of the loop.
    GetCount,
    /// Push the length of the loop's collection.
    GetLen,
    /// Push the accumulator of the loop.
    GetAcc,
    /// Pop into the accumulator of the loop.
    SetAcc,
    /// Pop into the index of the loop.
    SetIndex,
    /// Push the item of the loop.
    Pointer,
    /// Pop an error message and fail with it.
    Throw,
    /// Make the accumulator of a builtin: 1 for a group, 2 for a sort.
    Create(u8),
    /// Put the item under its key in the group.
    GroupBy,
    /// Add the item and its sort value.
    SortBy,
    /// Sort the items of the loop by their sort values.
    Sort,
    /// Start a loop over the top value.
    Begin,
    /// Pop two bools, push their conjunction.
    And,
    /// Pop two bools, push their disjunction.
    Or,
    /// End a loop.
    End,
}

/// The state of one loop of a builtin.
struct Scope {
    index: i64,
    len: i64,
    count: i64,
    acc: Value,
    items: Arc<Vec<Value>>,
    sort: Option<SortState>,
}

/// What `sortBy` collects: the items with their values, and the order.
struct SortState {
    desc: bool,
    array: Vec<Value>,
    values: Vec<Value>,
}

/// Runs `program` on `env`, a map of the names it may use.
pub fn run(program: &Program, env: &Value) -> Result<Value, file::Error> {
    let mut vm = Vm {
        stack: Vec::new(),
        scopes: Vec::new(),
        variables: vec![Value::Nil; program.variables],
        memory: 0,
        budget: DEFAULT_MEMORY_BUDGET,
    };
    let mut ip = 0;
    while ip < program.instrs.len() {
        let instr = &program.instrs[ip];
        ip += 1;
        if let Err(message) = vm.step(program, env, instr.op, &mut ip) {
            let location = program.instrs[ip - 1].location;
            return Err(file::Error::new(location, message).bind(&program.source));
        }
    }
    Ok(vm.stack.pop().unwrap_or(Value::Nil))
}

struct Vm {
    stack: Vec<Value>,
    scopes: Vec<Scope>,
    variables: Vec<Value>,
    memory: u64,
    budget: u64,
}

pub(crate) fn assertion_failed(value: &Value, want: &str) -> Panic {
    format!(
        "interface conversion: interface {{}} is {}, not {want}",
        value.type_name()
    )
}

fn as_bool(value: &Value) -> Result<bool, Panic> {
    match value {
        Value::Bool(b) => Ok(*b),
        other => Err(assertion_failed(other, "bool")),
    }
}

fn as_int(value: &Value) -> Result<i64, Panic> {
    match value {
        Value::Int(i) => Ok(*i),
        other => Err(assertion_failed(other, "int")),
    }
}

fn as_str(value: &Value) -> Result<&[u8], Panic> {
    match value {
        Value::Str(s) => Ok(s),
        other => Err(assertion_failed(other, "string")),
    }
}

impl Vm {
    fn push(&mut self, value: Value) {
        self.stack.push(value);
    }

    fn pop(&mut self) -> Result<Value, Panic> {
        self.stack.pop().ok_or_else(|| "stack underflow".to_owned())
    }

    fn current(&self) -> Result<&Value, Panic> {
        self.stack
            .last()
            .ok_or_else(|| "stack underflow".to_owned())
    }

    fn mem_grow(&mut self, size: u64) -> Result<(), Panic> {
        self.memory += size;
        if self.memory >= self.budget {
            return Err("memory budget exceeded".to_owned());
        }
        Ok(())
    }

    fn scope(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("a loop is running")
    }

    fn binary(&mut self, op: impl Fn(&Value, &Value) -> Result<Value, Panic>) -> Result<(), Panic> {
        let b = self.pop()?;
        let a = self.pop()?;
        let value = op(&a, &b)?;
        self.push(value);
        Ok(())
    }

    fn compare(&mut self, op: impl Fn(&Value, &Value) -> Result<bool, Panic>) -> Result<(), Panic> {
        let b = self.pop()?;
        let a = self.pop()?;
        let value = op(&a, &b)?;
        self.push(Value::Bool(value));
        Ok(())
    }

    fn step(
        &mut self,
        program: &Program,
        env: &Value,
        op: Op,
        ip: &mut usize,
    ) -> Result<(), Panic> {
        match op {
            Op::Push(index) => {
                let value = match &program.constants[index] {
                    Constant::Value(value) => value.clone(),
                    Constant::Error(message) => Value::str(message),
                    Constant::Name(name) => Value::str(name),
                    _ => return Err("invalid constant for push".to_owned()),
                };
                self.push(value);
            }
            Op::Int(i) => self.push(Value::Int(i)),
            Op::Pop => {
                self.pop()?;
            }
            Op::Store(index) => {
                let value = self.pop()?;
                self.variables[index] = value;
            }
            Op::LoadVar(index) => {
                let value = self.variables[index].clone();
                self.push(value);
            }
            Op::LoadConst(index) => {
                let Constant::Name(name) = &program.constants[index] else {
                    return Err("invalid name constant".to_owned());
                };
                let key = Value::str(name);
                let value = value::fetch(env, &key)?;
                self.push(value);
            }
            Op::LoadField(index) => {
                let Constant::Field(field) = &program.constants[index] else {
                    return Err("invalid field constant".to_owned());
                };
                let value = value::fetch_field(env, &field.index, &field.path)?;
                self.push(value);
            }
            Op::LoadFast(index) => {
                let Constant::Name(name) = &program.constants[index] else {
                    return Err("invalid name constant".to_owned());
                };
                let Value::Map(_, entries) = env else {
                    return Err(assertion_failed(env, "map[string]interface {}"));
                };
                let value = entries
                    .iter()
                    .find(|(k, _)| k.as_str() == Some(name.as_bytes()))
                    .map_or(Value::Nil, |(_, v)| v.clone());
                self.push(value);
            }
            Op::LoadMethod(index) => {
                let Constant::Method(method) = &program.constants[index] else {
                    return Err("invalid method constant".to_owned());
                };
                let value = value::fetch_method(env, &method.name)?;
                self.push(value);
            }
            Op::LoadEnv => self.push(env.clone()),
            Op::Fetch => {
                let b = self.pop()?;
                let a = self.pop()?;
                let value = value::fetch(&a, &b)?;
                self.push(value);
            }
            Op::FetchField(index) => {
                let a = self.pop()?;
                let Constant::Field(field) = &program.constants[index] else {
                    return Err("invalid field constant".to_owned());
                };
                let value = value::fetch_field(&a, &field.index, &field.path)?;
                self.push(value);
            }
            Op::Method(index) => {
                let a = self.pop()?;
                let Constant::Method(method) = &program.constants[index] else {
                    return Err("invalid method constant".to_owned());
                };
                let value = value::fetch_method(&a, &method.name)?;
                self.push(value);
            }
            Op::True => self.push(Value::Bool(true)),
            Op::False => self.push(Value::Bool(false)),
            Op::Nil => self.push(Value::Nil),
            Op::Negate => {
                let v = self.pop()?;
                let value = value::negate(&v)?;
                self.push(value);
            }
            Op::Not => {
                let v = self.pop()?;
                let b = as_bool(&v)?;
                self.push(Value::Bool(!b));
            }
            Op::Equal => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(Value::Bool(value::equal(&a, &b)));
            }
            Op::Jump(offset) => *ip = jump(*ip, offset)?,
            Op::JumpIfTrue(offset) => {
                if as_bool(self.current()?)? {
                    *ip = jump(*ip, offset)?;
                }
            }
            Op::JumpIfFalse(offset) => {
                if !as_bool(self.current()?)? {
                    *ip = jump(*ip, offset)?;
                }
            }
            Op::JumpIfNil(offset) => {
                if self.current()?.is_nil() {
                    *ip = jump(*ip, offset)?;
                }
            }
            Op::JumpIfNotNil(offset) => {
                if !self.current()?.is_nil() {
                    *ip = jump(*ip, offset)?;
                }
            }
            Op::JumpIfEnd(offset) => {
                let scope = self.scope();
                if scope.index >= scope.len {
                    *ip = jump(*ip, offset)?;
                }
            }
            Op::JumpBackward(offset) => *ip -= offset as usize,
            Op::In => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(Value::Bool(value::in_(&a, &b)?));
            }
            Op::Less => self.compare(value::less)?,
            Op::More => self.compare(value::more)?,
            Op::LessOrEqual => self.compare(value::less_or_equal)?,
            Op::MoreOrEqual => self.compare(value::more_or_equal)?,
            Op::Add => self.binary(value::add)?,
            Op::Subtract => self.binary(value::subtract)?,
            Op::Multiply => self.binary(value::multiply)?,
            Op::Divide => self.binary(value::divide)?,
            Op::Modulo => self.binary(value::modulo)?,
            Op::Exponent => self.binary(value::exponent)?,
            Op::Range => {
                let b = self.pop()?;
                let a = self.pop()?;
                let min = value::to_int(&a)?;
                let max = value::to_int(&b)?;
                let size = (max - min + 1).max(0);
                self.mem_grow(size as u64)?;
                self.push(value::make_range(min, max));
            }
            Op::Matches => {
                let b = self.pop()?;
                let a = self.pop()?;
                if a.is_nil() || b.is_nil() {
                    self.push(Value::Bool(false));
                    return Ok(());
                }
                let pattern = String::from_utf8_lossy(as_str(&b)?).into_owned();
                let regex = crate::regexp::compile(&pattern)?;
                let text = match &a {
                    Value::Str(s) | Value::Bytes(s) => s,
                    other => return Err(assertion_failed(other, "string")),
                };
                self.push(Value::Bool(regex.is_match(text)));
            }
            Op::MatchesConst(index) => {
                let a = self.pop()?;
                if a.is_nil() {
                    self.push(Value::Bool(false));
                    return Ok(());
                }
                let Constant::Regex(regex) = &program.constants[index] else {
                    return Err("invalid regexp constant".to_owned());
                };
                let text = match &a {
                    Value::Str(s) | Value::Bytes(s) => s,
                    other => return Err(assertion_failed(other, "[]uint8")),
                };
                self.push(Value::Bool(regex.is_match(text)));
            }
            Op::Contains | Op::StartsWith | Op::EndsWith => {
                let b = self.pop()?;
                let a = self.pop()?;
                if a.is_nil() || b.is_nil() {
                    self.push(Value::Bool(false));
                    return Ok(());
                }
                let text = as_str(&a)?;
                let part = as_str(&b)?;
                let found = match op {
                    Op::Contains => contains(text, part),
                    Op::StartsWith => text.starts_with(part),
                    _ => text.ends_with(part),
                };
                self.push(Value::Bool(found));
            }
            Op::Slice => {
                let from = self.pop()?;
                let to = self.pop()?;
                let node = self.pop()?;
                let value = value::slice(&node, &from, &to)?;
                self.push(value);
            }
            Op::Call(argc) => {
                let callee = self.pop()?;
                let args = self.pop_args(argc)?;
                let value = match callee {
                    Value::Nil => return Err("invalid operation: cannot call nil".to_owned()),
                    Value::Method(receiver, name) => value::call_method(&receiver, name, &args)?,
                    other => {
                        return Err(format!(
                            "invalid operation: cannot call non-function of type {}",
                            other.type_name()
                        ));
                    }
                };
                self.push(value);
            }
            Op::CallFunc(index, argc) => {
                let args = self.pop_args(argc)?;
                let f = program.functions[index];
                let call = f
                    .func
                    .ok_or_else(|| format!("{} has no plain call", f.name))?;
                let value = call(&args)?;
                self.push(value);
            }
            Op::CallSafe(index, argc) => {
                let args = self.pop_args(argc)?;
                let f = program.functions[index];
                let call = f
                    .safe
                    .ok_or_else(|| format!("{} has no safe call", f.name))?;
                let (value, mem) = call(&args)?;
                self.mem_grow(mem)?;
                self.push(value);
            }
            Op::Array => {
                let size = as_int(&self.pop()?)? as u64;
                self.mem_grow(size)?;
                let items = self.pop_args(size as usize)?;
                self.push(Value::any_array(items));
            }
            Op::Map => {
                let size = as_int(&self.pop()?)? as u64;
                self.mem_grow(size)?;
                let mut entries = Vec::with_capacity(size as usize);
                for _ in 0..size {
                    let value = self.pop()?;
                    let key = self.pop()?;
                    entries.push((key, value));
                }
                entries.reverse();
                let mut map: Vec<(Value, Value)> = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    as_str(&key)?;
                    match map
                        .iter_mut()
                        .find(|entry| value::key_identical(&entry.0, &key))
                    {
                        Some(entry) => entry.1 = value,
                        None => map.push((key, value)),
                    }
                }
                self.push(Value::string_map(map));
            }
            Op::Len => {
                let length = value::len(self.current()?)?;
                self.push(Value::Int(length));
            }
            Op::IncrementIndex => self.scope().index += 1,
            Op::DecrementIndex => self.scope().index -= 1,
            Op::IncrementCount => self.scope().count += 1,
            Op::GetIndex => {
                let index = self.scope().index;
                self.push(Value::Int(index));
            }
            Op::GetCount => {
                let count = self.scope().count;
                self.push(Value::Int(count));
            }
            Op::GetLen => {
                let len = self.scope().len;
                self.push(Value::Int(len));
            }
            Op::GetAcc => {
                let acc = self.scope().acc.clone();
                self.push(acc);
            }
            Op::SetAcc => {
                let acc = self.pop()?;
                self.scope().acc = acc;
            }
            Op::SetIndex => {
                let index = as_int(&self.pop()?)?;
                self.scope().index = index;
            }
            Op::Pointer => {
                let item = self.scope_item()?;
                self.push(item);
            }
            Op::Throw => {
                let message = self.pop()?;
                return Err(String::from_utf8_lossy(as_str(&message)?).into_owned());
            }
            Op::Create(kind) => match kind {
                1 => {
                    let groups = Value::Map(
                        Ty::map(Ty::any(), Ty::slice(Ty::any())),
                        Arc::new(Vec::new()),
                    );
                    self.push(groups);
                }
                2 => {
                    let order = self.pop()?;
                    let Value::Str(order) = order else {
                        return Err("sortBy order argument must be a string".to_owned());
                    };
                    let desc = match &order[..] {
                        b"asc" => false,
                        b"desc" => true,
                        _ => return Err("unknown order, use asc or desc".to_owned()),
                    };
                    self.scope().sort = Some(SortState {
                        desc,
                        array: Vec::new(),
                        values: Vec::new(),
                    });
                    self.push(Value::Nil);
                }
                other => return Err(format!("unknown OpCreate argument {other}")),
            },
            Op::GroupBy => {
                let key = self.pop()?;
                let item = self.scope_item()?;
                let Value::Map(_, entries) = &mut self.scope().acc else {
                    return Err("group accumulator is not a map".to_owned());
                };
                let entries = Arc::make_mut(entries);
                match entries
                    .iter_mut()
                    .find(|entry| value::key_identical(&entry.0, &key))
                {
                    Some((_, Value::Array(_, group))) => Arc::make_mut(group).push(item),
                    Some(entry) => entry.1 = Value::any_array(vec![item]),
                    None => entries.push((
                        key,
                        Value::Array(Ty::slice(Ty::any()), Arc::new(vec![item])),
                    )),
                }
            }
            Op::SortBy => {
                let value = self.pop()?;
                let item = self.scope_item()?;
                let sort = self
                    .scope()
                    .sort
                    .as_mut()
                    .ok_or_else(|| "sortBy accumulator is missing".to_owned())?;
                sort.array.push(item);
                sort.values.push(value);
            }
            Op::Sort => {
                let mem = self.scopes.last().map_or(0, |s| s.len as u64);
                let sort = self
                    .scope()
                    .sort
                    .take()
                    .ok_or_else(|| "sortBy accumulator is missing".to_owned())?;
                let mut sortable = Sortable {
                    desc: sort.desc,
                    array: sort.array,
                    values: sort.values,
                    error: RefCell::new(None),
                };
                gosort::sort(&mut sortable);
                if let Some(err) = sortable.error.take() {
                    return Err(err);
                }
                self.mem_grow(mem)?;
                self.push(Value::any_array(sortable.array));
            }
            Op::Begin => {
                let collection = self.pop()?;
                let (items, len) = match collection {
                    Value::Array(_, items) => {
                        let len = items.len() as i64;
                        (items, len)
                    }
                    other => {
                        return Err(format!(
                            "reflect: call of reflect.Value.Len on {} Value",
                            if other.is_nil() { "zero" } else { "non-array" }
                        ));
                    }
                };
                self.scopes.push(Scope {
                    index: 0,
                    len,
                    count: 0,
                    acc: Value::Nil,
                    items,
                    sort: None,
                });
            }
            Op::And => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.push(Value::Bool(as_bool(&a)? && as_bool(&b)?));
            }
            Op::Or => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.push(Value::Bool(as_bool(&a)? || as_bool(&b)?));
            }
            Op::End => {
                self.scopes.pop();
            }
        }
        Ok(())
    }

    fn scope_item(&mut self) -> Result<Value, Panic> {
        let scope = self.scopes.last().ok_or_else(|| "no loop".to_owned())?;
        scope
            .items
            .get(scope.index as usize)
            .cloned()
            .ok_or_else(|| format!("reflect: Value.Index: index {} out of range", scope.index))
    }

    fn pop_args(&mut self, count: usize) -> Result<Vec<Value>, Panic> {
        if self.stack.len() < count {
            return Err("stack underflow".to_owned());
        }
        let at = self.stack.len() - count;
        Ok(self.stack.split_off(at))
    }
}

/// `vm.ip += arg`, with Go's check on a negative offset.
fn jump(ip: usize, offset: i64) -> Result<usize, Panic> {
    if offset < 0 {
        return Err("negative jump offset is invalid".to_owned());
    }
    Ok(ip + offset as usize)
}

fn contains(text: &[u8], part: &[u8]) -> bool {
    part.is_empty() || text.windows(part.len()).any(|w| w == part)
}

/// The items of a `sortBy`, ordered by their values, as Go's `SortBy` sorts.
/// A comparison that panics records the panic and reports the items as not
/// ordered; the first panic is the error of the sort, as in Go.
struct Sortable {
    desc: bool,
    array: Vec<Value>,
    values: Vec<Value>,
    error: RefCell<Option<Panic>>,
}

impl gosort::Interface for Sortable {
    fn len(&self) -> usize {
        self.array.len()
    }

    fn less(&self, i: usize, j: usize) -> bool {
        let (a, b) = (&self.values[i], &self.values[j]);
        let (x, y) = if self.desc { (b, a) } else { (a, b) };
        match value::less(x, y) {
            Ok(holds) => holds,
            Err(message) => {
                self.error.borrow_mut().get_or_insert(message);
                false
            }
        }
    }

    fn swap(&mut self, i: usize, j: usize) {
        self.array.swap(i, j);
        self.values.swap(i, j);
    }
}
