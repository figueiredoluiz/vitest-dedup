//! What an expression's value may be, from a handful of JavaScript operations over
//! abstract values. Syntax is read in one place, [`Module::lower`]; every rule after it reads
//! operations and values, never node kinds. The framework enters only through [`Atom::Api`],
//! whose members and calls the dialect decides; JavaScript's own property tables decide every
//! other receiver.

use super::{Module, Origin, Step};
use crate::inventory::{Export, Modules, Reach, Registers, Root, Summary};
use crate::lang::{
    bound_symbols, export_name, member_name, property_name, Declaration, Field, Name, NodeId,
    SymbolId,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Whether a primitive is truthy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Truth {
    Yes,
    No,
    Either,
}

/// A primitive's class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Primitive {
    String,
    Number,
    Boolean,
    /// Another primitive, or one whose class is not known (`a + b`).
    Other,
}

/// One value an expression may evaluate to.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Atom<'s> {
    Undefined,
    Null,
    Primitive(Primitive, Truth),
    /// An array literal, by its node.
    Array(NodeId),
    /// An object literal, by its node.
    Object(NodeId),
    /// A function of this module, by its node.
    Function(NodeId),
    /// A project module's namespace object, by the import binding.
    Module(SymbolId),
    /// A framework value: a root the dialect knows and the steps applied to it.
    Api(Origin, Vec<Step<'s>>),
    /// A built-in function or object: truthy, never nullish, never a registration.
    Builtin,
    /// Anything.
    Unknown,
}

impl Atom<'_> {
    fn may_be_truthy(&self) -> bool {
        !matches!(
            self,
            Atom::Undefined | Atom::Null | Atom::Primitive(_, Truth::No)
        )
    }

    fn may_be_falsy(&self) -> bool {
        matches!(
            self,
            Atom::Undefined
                | Atom::Null
                | Atom::Primitive(_, Truth::No | Truth::Either)
                | Atom::Unknown
        )
    }

    fn may_be_nullish(&self) -> bool {
        matches!(self, Atom::Undefined | Atom::Null | Atom::Unknown)
    }

    fn may_be_other_than_nullish(&self) -> bool {
        !matches!(self, Atom::Undefined | Atom::Null)
    }
}

/// The values an expression may evaluate to; empty when it always throws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Value<'s>(Vec<Atom<'s>>);

/// More alternatives than this widen to [`Atom::Unknown`].
const WIDEST: usize = 16;
/// Nested evaluations past this depth are [`Atom::Unknown`].
const DEEPEST: usize = 48;

impl<'s> Value<'s> {
    fn of(atom: Atom<'s>) -> Self {
        Value(vec![atom])
    }

    fn unknown() -> Self {
        Value::of(Atom::Unknown)
    }

    fn join(mut self, other: Value<'s>) -> Self {
        for atom in other.0 {
            if !self.0.contains(&atom) {
                self.0.push(atom);
            }
        }
        if self.0.len() > WIDEST {
            return Value::unknown();
        }
        self
    }

    fn filter(self, keep: impl Fn(&Atom<'s>) -> bool) -> Self {
        Value(self.0.into_iter().filter(|atom| keep(atom)).collect())
    }

    /// The alternatives that pass a truth test (`passing`) or fail it, a primitive of either
    /// truth narrowed to the outcome.
    fn narrowed(self, guard: Guard, passing: bool) -> Self {
        let keeps = |atom: &Atom<'s>| match (guard, passing) {
            (Guard::Truthy, true) => atom.may_be_truthy(),
            (Guard::Truthy, false) => atom.may_be_falsy(),
            (Guard::NotNullish, true) => atom.may_be_other_than_nullish(),
            (Guard::NotNullish, false) => atom.may_be_nullish(),
        };
        Value(
            self.0
                .into_iter()
                .filter(keeps)
                .map(|atom| match (guard, atom) {
                    (Guard::Truthy, Atom::Primitive(class, Truth::Either)) => {
                        Atom::Primitive(class, truth(passing))
                    }
                    (_, atom) => atom,
                })
                .collect(),
        )
    }

    /// Whether evaluating it always throws.
    fn throws(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn holds_api(&self) -> bool {
        self.0.iter().any(|atom| matches!(atom, Atom::Api(..)))
    }
}

/// The key a property read uses.
#[derive(Clone, Copy, Debug)]
enum Key<'s> {
    Name(&'s str),
    Index(usize),
    Dynamic,
}

/// Which outcome of its test a choice takes the first branch on.
#[derive(Clone, Copy, Debug)]
pub(super) enum Guard {
    Truthy,
    NotNullish,
}

/// The operations every expression lowers to.
#[derive(Debug)]
enum Op<'s> {
    /// A value fixed by its syntax: a literal, a container, a function.
    Is(Atom<'s>),
    /// `then` when `test` passes `guard`, else `otherwise`; a missing branch is the test's own
    /// value (`a && b` is `a` when `a` is falsy).
    Choose {
        test: NodeId,
        guard: Guard,
        then: Option<NodeId>,
        otherwise: Option<NodeId>,
    },
    /// The value of `value`, after evaluating whatever precedes it.
    Last(NodeId),
    /// A property of `object`.
    Get(NodeId, Key<'s>),
    /// What calling `callee` with `arguments` returns.
    Call(NodeId, Vec<NodeId>),
    /// A binding's value.
    Read(NodeId),
    /// What awaiting `value` resolves to.
    Await(NodeId),
    /// Each expression in order, the last one's value: nothing when an earlier one throws.
    Sequence(Vec<NodeId>),
    /// Syntax with no lowering.
    Unknown,
}

/// What calling a framework value does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    Registers(Registers),
    /// A project wrapper: it may register anything.
    Wrapper,
    /// The API's namespace, which is not called.
    Namespace,
    /// A value the dialect does not register through: a builder not called yet, or a step past
    /// the API.
    Other,
}

/// Parameter bindings while a local function's result is evaluated.
type Scope<'s> = HashMap<SymbolId, Value<'s>>;

/// A spike-wide record of the namespace imports of project modules.
pub(super) type Namespaces = HashMap<SymbolId, HashMap<String, Summary>>;

/// A function's own properties, then its prototype's, as JavaScript defines them.
const FUNCTION_MEMBERS: [&str; 4] = ["call", "apply", "bind", "toString"];
/// A string's prototype methods most read.
const STRING_MEMBERS: [&str; 8] = [
    "toUpperCase",
    "toLowerCase",
    "slice",
    "split",
    "trim",
    "includes",
    "replace",
    "toString",
];
/// A number's and a boolean's prototype methods.
const NUMBER_MEMBERS: [&str; 4] = ["toFixed", "toString", "valueOf", "toPrecision"];
/// An array's prototype methods most read.
const ARRAY_MEMBERS: [&str; 8] = [
    "map", "filter", "forEach", "find", "push", "concat", "join", "toString",
];
/// `Object.prototype`'s methods.
const OBJECT_MEMBERS: [&str; 4] = ["toString", "valueOf", "hasOwnProperty", "constructor"];

/// What `key` reads on a value whose own properties are `own` and whose prototype's methods are
/// `methods`: a method is a built-in function, anything else is `undefined`.
fn table<'s>(key: Key<'s>, own: &[(&str, Atom<'s>)], methods: &[&str]) -> Value<'s> {
    match key {
        Key::Name(name) => {
            if let Some((_, atom)) = own.iter().find(|(own, _)| *own == name) {
                Value::of(atom.clone())
            } else if methods.contains(&name) || OBJECT_MEMBERS.contains(&name) {
                Value::of(Atom::Builtin)
            } else {
                Value::of(Atom::Undefined)
            }
        }
        Key::Index(_) => Value::of(Atom::Undefined),
        Key::Dynamic => Value::unknown(),
    }
}

/// Evaluation state for one module.
#[derive(Default)]
pub(super) struct Flow<'s> {
    memo: RefCell<HashMap<NodeId, Value<'s>>>,
    active: RefCell<Vec<NodeId>>,
}

impl<'s> Flow<'s> {
    /// Forgets every evaluation: what the module knows of its bindings changed.
    pub(super) fn clear(&self) {
        self.memo.borrow_mut().clear();
    }
}

impl<'s> Module<'s> {
    /// The only reading of syntax: which operation `node` is.
    fn lower(&self, node: NodeId) -> Op<'s> {
        let tree = self.tree;
        let child = |key| tree.child(node, key);
        if tree.kind(node) == "MemberExpression" {
            if let Some((origin, _)) = self.root_origin(node) {
                return Op::Is(Atom::Api(origin, Vec::new()));
            }
        }
        match tree.kind(node) {
            "Identifier" => Op::Read(node),
            "Literal" => Op::Is(match tree.field_of(node, "value") {
                Field::Value(serde_json::Value::Null) => Atom::Null,
                Field::Value(serde_json::Value::Bool(value)) => {
                    Atom::Primitive(Primitive::Boolean, truth(*value))
                }
                Field::Value(serde_json::Value::Number(number)) => {
                    let value = number.as_f64().unwrap_or(f64::NAN);
                    Atom::Primitive(Primitive::Number, truth(value != 0.0 && !value.is_nan()))
                }
                Field::Value(serde_json::Value::String(text)) => {
                    Atom::Primitive(Primitive::String, truth(!text.is_empty()))
                }
                // A regular expression or a bigint.
                _ => Atom::Builtin,
            }),
            "TemplateLiteral" => Op::Is(Atom::Primitive(Primitive::String, Truth::Either)),
            "ArrayExpression" => Op::Is(Atom::Array(node)),
            "ObjectExpression" => Op::Is(Atom::Object(node)),
            "ArrowFunctionExpression" | "FunctionExpression" => Op::Is(Atom::Function(node)),
            "ClassExpression" | "NewExpression" => Op::Is(Atom::Builtin),
            "ConditionalExpression" => {
                match (child("test"), child("consequent"), child("alternate")) {
                    (Some(test), Some(then), Some(otherwise)) => Op::Choose {
                        test,
                        guard: Guard::Truthy,
                        then: Some(then),
                        otherwise: Some(otherwise),
                    },
                    _ => Op::Unknown,
                }
            }
            "LogicalExpression" => match (child("left"), child("right")) {
                (Some(left), Some(right)) => match tree.text(node, "operator") {
                    Some("&&") => Op::Choose {
                        test: left,
                        guard: Guard::Truthy,
                        then: Some(right),
                        otherwise: None,
                    },
                    Some("||") => Op::Choose {
                        test: left,
                        guard: Guard::Truthy,
                        then: None,
                        otherwise: Some(right),
                    },
                    Some("??") => Op::Choose {
                        test: left,
                        guard: Guard::NotNullish,
                        then: None,
                        otherwise: Some(right),
                    },
                    _ => Op::Unknown,
                },
                _ => Op::Unknown,
            },
            "SequenceExpression" => {
                let expressions: Vec<NodeId> = tree
                    .list(node, "expressions")
                    .into_iter()
                    .flatten()
                    .collect();
                if expressions.is_empty() {
                    Op::Unknown
                } else {
                    Op::Sequence(expressions)
                }
            }
            "AssignmentExpression" if tree.text(node, "operator") == Some("=") => {
                child("right").map_or(Op::Unknown, Op::Last)
            }
            "MemberExpression" => {
                let Some(object) = child("object") else {
                    return Op::Unknown;
                };
                let key = match member_name(tree, node) {
                    Some(name) => Key::Name(name),
                    None => child("property")
                        .map(|property| tree.inner(property))
                        .and_then(|property| match tree.field_of(property, "value") {
                            Field::Value(serde_json::Value::Number(number)) => {
                                number.as_u64().map(|index| Key::Index(index as usize))
                            }
                            _ => None,
                        })
                        .unwrap_or(Key::Dynamic),
                };
                Op::Get(object, key)
            }
            "CallExpression" => match child("callee") {
                Some(callee) => Op::Call(
                    callee,
                    tree.list(node, "arguments").into_iter().flatten().collect(),
                ),
                None => Op::Unknown,
            },
            "TaggedTemplateExpression" => {
                child("tag").map_or(Op::Unknown, |tag| Op::Call(tag, vec![]))
            }
            "AwaitExpression" => child("argument").map_or(Op::Unknown, Op::Await),
            "ChainExpression" => child("expression").map_or(Op::Unknown, Op::Last),
            "UnaryExpression" => Op::Is(match tree.text(node, "operator") {
                Some("void") => Atom::Undefined,
                Some("typeof") => Atom::Primitive(Primitive::String, Truth::Yes),
                Some("!") => Atom::Primitive(Primitive::Boolean, Truth::Either),
                _ => Atom::Primitive(Primitive::Number, Truth::Either),
            }),
            "BinaryExpression" => Op::Is(match tree.text(node, "operator") {
                Some("+") => Atom::Primitive(Primitive::Other, Truth::Either),
                Some(
                    "==" | "!=" | "===" | "!==" | "<" | ">" | "<=" | ">=" | "in" | "instanceof",
                ) => Atom::Primitive(Primitive::Boolean, Truth::Either),
                _ => Atom::Primitive(Primitive::Number, Truth::Either),
            }),
            _ => Op::Unknown,
        }
    }

    /// What `node` may evaluate to.
    pub(super) fn evaluate(&self, node: NodeId) -> Value<'s> {
        // What the module knows of its bindings may have grown since the last evaluation.
        self.flow.clear();
        self.evaluate_in(node, &Scope::new(), 0)
    }

    fn evaluate_in(&self, node: NodeId, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let node = self.tree.inner(node);
        #[cfg(test)]
        super::WALK_STEPS.with(|steps| steps.set(steps.get() + 1));
        if depth > DEEPEST || self.flow.active.borrow().contains(&node) {
            return Value::unknown();
        }
        let memoised = scope.is_empty();
        if memoised {
            if let Some(value) = self.flow.memo.borrow().get(&node) {
                return value.clone();
            }
        }
        self.flow.active.borrow_mut().push(node);
        let value = self.apply_op(self.lower(node), scope, depth + 1);
        self.flow.active.borrow_mut().pop();
        if memoised {
            self.flow.memo.borrow_mut().insert(node, value.clone());
        }
        value
    }

    /// The rule of each operation.
    fn apply_op(&self, op: Op<'s>, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let evaluate = |node| self.evaluate_in(node, scope, depth);
        match op {
            // A container literal evaluates every part: one that throws throws the literal.
            Op::Is(atom @ (Atom::Array(_) | Atom::Object(_))) => {
                if self.parts(&atom).any(|part| evaluate(part).throws()) {
                    Value::default()
                } else {
                    Value::of(atom)
                }
            }
            Op::Is(atom) => Value::of(atom),
            Op::Unknown => Value::unknown(),
            Op::Last(node) => evaluate(node),
            Op::Sequence(expressions) => {
                let (last, earlier) = expressions.split_last().expect("a sequence has a part");
                if earlier
                    .iter()
                    .any(|&expression| evaluate(expression).throws())
                {
                    Value::default()
                } else {
                    evaluate(*last)
                }
            }
            Op::Choose {
                test,
                guard,
                then,
                otherwise,
            } => {
                let tested = evaluate(test);
                let passed = tested.clone().narrowed(guard, true);
                let failed = tested.narrowed(guard, false);
                let mut value = Value::default();
                if !passed.throws() {
                    value = value.join(match then {
                        Some(then) => evaluate(then),
                        None => passed,
                    });
                }
                if !failed.throws() {
                    value = value.join(match otherwise {
                        Some(otherwise) => evaluate(otherwise),
                        None => failed,
                    });
                }
                value
            }
            Op::Await(node) => {
                let awaited = evaluate(node);
                if awaited.throws() {
                    Value::default()
                } else if awaited.holds_api() {
                    Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), Vec::new()))
                } else {
                    awaited
                        .filter(|atom| !matches!(atom, Atom::Function(_)))
                        .join(Value::unknown())
                }
            }
            Op::Get(object, key) => {
                let mut value = Value::default();
                for atom in evaluate(object).0 {
                    value = value.join(self.get(atom, key, scope, depth));
                }
                value
            }
            Op::Call(callee, arguments) => {
                let handed: Vec<Value<'s>> = arguments
                    .iter()
                    .map(|&argument| evaluate(argument))
                    .collect();
                // An argument that throws stops the call before it happens.
                if handed.iter().any(Value::throws) {
                    return Value::default();
                }
                let mut value = Value::default();
                for atom in evaluate(callee).0 {
                    value = value.join(self.call(atom, &arguments, &handed, depth));
                }
                value
            }
            Op::Read(identifier) => self.read_value(identifier, scope, depth),
        }
    }

    /// The expressions a container literal evaluates: its elements, spread arguments, computed
    /// keys and property values. A getter's or a method's body does not run.
    fn parts(&self, atom: &Atom<'s>) -> impl Iterator<Item = NodeId> + use<'_, 's> {
        let tree = self.tree;
        let (node, key) = match *atom {
            Atom::Array(node) => (Some(node), "elements"),
            Atom::Object(node) => (Some(node), "properties"),
            _ => (None, ""),
        };
        node.into_iter()
            .flat_map(move |node| tree.list(node, key).into_iter().flatten())
            .flat_map(move |part| {
                let (key, value) = match tree.kind(part) {
                    "SpreadElement" => (None, tree.child(part, "argument")),
                    "Property" => (
                        tree.flag(part, "computed")
                            .then(|| tree.child(part, "key"))
                            .flatten(),
                        (tree.text(part, "kind") == Some("init") && !tree.flag(part, "method"))
                            .then(|| tree.child(part, "value"))
                            .flatten(),
                    ),
                    _ => (None, Some(part)),
                };
                key.into_iter().chain(value)
            })
    }

    /// Reading `key` of one value.
    fn get(&self, atom: Atom<'s>, key: Key<'s>, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let tree = self.tree;
        let any_number = Atom::Primitive(Primitive::Number, Truth::Either);
        let any_string = Atom::Primitive(Primitive::String, Truth::Either);
        match atom {
            // Reading a property of `null` or `undefined` throws.
            Atom::Undefined | Atom::Null => Value::default(),
            Atom::Primitive(Primitive::String, _) => {
                table(key, &[("length", any_number)], &STRING_MEMBERS)
            }
            Atom::Primitive(Primitive::Number | Primitive::Boolean, _) => {
                table(key, &[], &NUMBER_MEMBERS)
            }
            Atom::Primitive(Primitive::Other, _) | Atom::Builtin | Atom::Unknown => match atom {
                Atom::Unknown => Value::unknown(),
                _ => Value::of(Atom::Builtin).join(Value::of(Atom::Undefined)),
            },
            Atom::Function(_) => table(
                key,
                &[("name", any_string), ("length", any_number)],
                &FUNCTION_MEMBERS,
            ),
            Atom::Array(node) => {
                let elements = tree.list(node, "elements");
                let spread = elements
                    .iter()
                    .flatten()
                    .any(|&element| tree.kind(element) == "SpreadElement");
                let element = |slot: &Option<NodeId>| match slot {
                    Some(element) => self.evaluate_in(*element, scope, depth),
                    None => Value::of(Atom::Undefined),
                };
                match key {
                    _ if spread => Value::unknown(),
                    Key::Index(index) => elements
                        .get(index)
                        .map_or(Value::of(Atom::Undefined), element),
                    Key::Name("length") => Value::of(Atom::Primitive(
                        Primitive::Number,
                        truth(!elements.is_empty()),
                    )),
                    Key::Name(name) => match name.parse::<usize>() {
                        Ok(index) => elements
                            .get(index)
                            .map_or(Value::of(Atom::Undefined), element),
                        Err(_) => table(key, &[], &ARRAY_MEMBERS),
                    },
                    Key::Dynamic => elements
                        .iter()
                        .fold(Value::of(Atom::Undefined), |value, slot| {
                            value.join(element(slot))
                        }),
                }
            }
            Atom::Object(node) => {
                let properties: Vec<NodeId> = tree
                    .list(node, "properties")
                    .into_iter()
                    .flatten()
                    .collect();
                let opaque = properties.iter().any(|&property| {
                    tree.kind(property) != "Property"
                        || tree.text(property, "kind") != Some("init")
                        || property_name(tree, property).is_none()
                });
                let value_of = |property: NodeId| match tree.child(property, "value") {
                    Some(value) => self.evaluate_in(value, scope, depth),
                    None => Value::unknown(),
                };
                match key {
                    _ if opaque => Value::unknown(),
                    Key::Name(name) => match properties
                        .iter()
                        .rev()
                        .find(|&&property| property_name(tree, property) == Some(name))
                    {
                        Some(&property) => value_of(property),
                        None => table(key, &[], &[]),
                    },
                    Key::Index(index) => {
                        let name = index.to_string();
                        match properties
                            .iter()
                            .rev()
                            .find(|&&property| property_name(tree, property) == Some(name.as_str()))
                        {
                            Some(&property) => value_of(property),
                            None => Value::of(Atom::Undefined),
                        }
                    }
                    Key::Dynamic => properties
                        .iter()
                        .fold(Value::of(Atom::Undefined), |value, &property| {
                            value.join(value_of(property))
                        }),
                }
            }
            Atom::Module(symbol) => match key {
                Key::Name(name) => match self
                    .namespaces
                    .get(&symbol)
                    .and_then(|exports| exports.get(name))
                {
                    Some(summary) => value_of_summary(summary),
                    // The module's other exports register nothing when called.
                    None => Value::of(Atom::Builtin).join(Value::of(Atom::Undefined)),
                },
                _ => Value::unknown(),
            },
            Atom::Api(origin, mut steps) => match key {
                // A function's own properties are JavaScript's; only the module namespace itself
                // is an object without them.
                Key::Name("name" | "length")
                    if !(origin == Origin::Namespace && steps.is_empty()) =>
                {
                    Value::of(if matches!(key, Key::Name("name")) {
                        any_string
                    } else {
                        any_number
                    })
                }
                // A wrapper is a project value: any member of it may hold anything.
                Key::Name(name) if origin.reach().is_some() => {
                    steps.push(Step::Member(name));
                    Value::of(Atom::Api(origin, steps))
                }
                // An element of the wrapper itself (`const [first] = setup`); one of what it
                // returns is not followed (`list()[0]`).
                _ if origin.reach().is_some() && !steps.contains(&Step::Call) => {
                    steps.push(Step::Member("[]"));
                    Value::of(Atom::Api(origin, steps))
                }
                Key::Name(name) => {
                    steps.push(Step::Member(name));
                    // A member the API does not have is not the API's: the dialect cannot say
                    // what it is (`v.expect`, `test.skip.unknown`).
                    if self.apply(origin, &steps).is_none() {
                        return Value::unknown();
                    }
                    Value::of(Atom::Api(origin, steps))
                }
                _ => Value::unknown(),
            },
        }
    }

    /// What calling one value with `arguments` returns.
    fn call(
        &self,
        atom: Atom<'s>,
        arguments: &[NodeId],
        handed: &[Value<'s>],
        depth: usize,
    ) -> Value<'s> {
        // A wrapper, or what it returns, is a project function: called, it may register, but a
        // callee handed it is not handed the framework's (`expect((await run()).stderr)`).
        let handed_api = handed.iter().flat_map(|value| &value.0).any(
            |atom| matches!(atom, Atom::Api(origin, _) if !matches!(origin, Origin::Wrapper(_))),
        );
        match atom {
            // Calling a primitive or a container throws.
            Atom::Undefined
            | Atom::Null
            | Atom::Primitive(..)
            | Atom::Array(_)
            | Atom::Object(_) => Value::default(),
            Atom::Api(origin, mut steps) => {
                steps.push(Step::Call);
                if origin.reach().is_none() && self.apply(origin, &steps).is_none() {
                    // A call that registers returns nothing that registers.
                    return Value::of(Atom::Builtin).join(Value::of(Atom::Undefined));
                }
                Value::of(Atom::Api(origin, steps))
            }
            Atom::Function(function) => self.returned(function, arguments, handed, depth),
            Atom::Builtin | Atom::Module(_) | Atom::Unknown if handed_api => {
                // What a call handed a registration function returns may register.
                Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), Vec::new()))
            }
            Atom::Builtin | Atom::Module(_) => {
                Value::of(Atom::Builtin).join(Value::of(Atom::Undefined))
            }
            Atom::Unknown => Value::unknown(),
        }
    }

    /// What a local function returns when called with `handed`: its expression body or its
    /// `return` arguments, with each plain parameter bound to its argument.
    fn returned(
        &self,
        function: NodeId,
        arguments: &[NodeId],
        handed: &[Value<'s>],
        depth: usize,
    ) -> Value<'s> {
        let tree = self.tree;
        let mut scope = Scope::new();
        for (index, parameter) in tree.list(function, "params").into_iter().enumerate() {
            let Some(parameter) = parameter else { continue };
            let value = handed
                .get(index)
                .cloned()
                .unwrap_or_else(|| Value::of(Atom::Undefined));
            match tree.kind(parameter) {
                "Identifier" => {
                    if let Some(symbol) = self.declares(parameter) {
                        scope.insert(symbol, value);
                    }
                }
                // A pattern or a default parameter: what it binds is not followed.
                _ => {
                    for symbol in crate::lang::bound_symbols(tree, &self.parsed.scopes, parameter) {
                        scope.insert(symbol, Value::unknown());
                    }
                }
            }
        }
        let _ = arguments;
        let Some(body) = tree.child(function, "body") else {
            return Value::unknown();
        };
        if tree.kind(body) != "BlockStatement" {
            return self.evaluate_in(body, &scope, depth);
        }
        let mut value = Value::of(Atom::Undefined);
        for node in tree.descendants(body) {
            if tree.kind(node) != "ReturnStatement" {
                continue;
            }
            let nested = tree
                .ancestors(node)
                .take_while(|&outer| outer != function)
                .any(|outer| tree.is_function(outer));
            if nested {
                continue;
            }
            value = value.join(match tree.child(node, "argument") {
                Some(argument) => self.evaluate_in(argument, &scope, depth),
                None => Value::of(Atom::Undefined),
            });
        }
        value
    }

    /// What a binding read at `identifier` holds.
    fn read_value(&self, identifier: NodeId, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        let symbol = match scopes.name_at(tree.span(identifier).start) {
            Some(Name::Reference(Some(symbol))) => symbol,
            Some(Name::Reference(None)) => {
                return match (self.root_origin(identifier), tree.text(identifier, "name")) {
                    (Some((origin, _)), _) => Value::of(Atom::Api(origin, Vec::new())),
                    (None, Some("undefined")) => Value::of(Atom::Undefined),
                    _ => Value::unknown(),
                };
            }
            _ => return Value::unknown(),
        };
        if let Some(value) = scope.get(&symbol) {
            return value.clone();
        }
        if self.namespaces.contains_key(&symbol) {
            return Value::of(Atom::Module(symbol));
        }
        if let Some(summary) = self.imported.get(&symbol) {
            return value_of_summary(summary);
        }
        if let Some(&reach) = self.wrappers.get(&symbol) {
            return Value::of(Atom::Api(Origin::Wrapper(reach), Vec::new()));
        }
        // An import, or a binding the alias pass already resolved to a framework value: an alias
        // proved to hold one framework value is a root of its own (`const group = test.describe`).
        if let Some(&origin) = self.origins.get(&symbol) {
            if !self.aliased.contains(&symbol) {
                return Value::of(Atom::Api(origin, Vec::new()));
            }
            let value = self.symbol_value(symbol, scope, depth);
            let settled = self.resolve(&value).is_some_and(|(held, steps)| {
                match self.verdict(held, &steps) {
                    Verdict::Registers(_) | Verdict::Namespace => true,
                    Verdict::Wrapper => origin.reach().is_some(),
                    Verdict::Other => false,
                }
            });
            if settled && value.0.iter().all(|atom| matches!(atom, Atom::Api(..))) {
                return Value::of(Atom::Api(origin, Vec::new()));
            }
            return value;
        }
        self.symbol_value(symbol, scope, depth)
    }

    /// What `symbol` may hold wherever it is read: what each of its declarations binds, joined
    /// with every value written to it, and anything at all in a module that runs `eval`.
    fn symbol_value(&self, symbol: SymbolId, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let scopes = &self.parsed.scopes;
        let mut value = if self.redeclared.contains(&symbol) {
            self.redeclared_value(symbol, scope, depth)
        } else {
            match scopes.symbol(symbol).declaration.clone() {
                Declaration::Function(node) => Value::of(Atom::Function(node)),
                Declaration::Class(_) => Value::of(Atom::Builtin),
                Declaration::Variable(node) => self.declared_value(node, symbol, scope, depth),
                _ => Value::unknown(),
            }
        };
        for reference in crate::lang::proof::writes(scopes, symbol) {
            value = value.join(match reference.node {
                Some(target) => self.written_value(target, symbol, scope, depth),
                None => Value::unknown(),
            });
        }
        if self.evaluates {
            value = value.join(Value::unknown());
        }
        value
    }

    /// What every declaration of a symbol declared more than once binds: each declarator's value,
    /// each function, and anything for a parameter or a catch binding.
    fn redeclared_value(&self, symbol: SymbolId, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        let binds = |pattern: Option<NodeId>| {
            pattern.is_some_and(|pattern| bound_symbols(tree, scopes, pattern).contains(&symbol))
        };
        let mut value = Value::default();
        for node in tree.nodes() {
            match tree.kind(node) {
                "VariableDeclarator" if binds(tree.child(node, "id")) => {
                    value = value.join(self.declared_value(node, symbol, scope, depth));
                }
                "FunctionDeclaration" if binds(tree.child(node, "id")) => {
                    value = value.join(Value::of(Atom::Function(node)));
                }
                "CatchClause" if binds(tree.child(node, "param")) => {
                    value = value.join(Value::unknown());
                }
                _ if tree.is_function(node)
                    && tree
                        .list(node, "params")
                        .into_iter()
                        .flatten()
                        .any(|param| binds(Some(param))) =>
                {
                    value = value.join(Value::unknown());
                }
                _ => {}
            }
        }
        value
    }

    /// What the write of `symbol` at `target` stores: the assigned value projected through the
    /// pattern holding `target`, an element of what a `for ... of` iterates, a key for `for ... in`
    /// and a number for an update. A declarator's own write is its declaration's.
    fn written_value(
        &self,
        target: NodeId,
        symbol: SymbolId,
        scope: &Scope<'s>,
        depth: usize,
    ) -> Value<'s> {
        let tree = self.tree;
        let Some(writer) = tree.ancestors(target).find(|&node| {
            matches!(
                tree.kind(node),
                "AssignmentExpression"
                    | "UpdateExpression"
                    | "ForOfStatement"
                    | "ForInStatement"
                    | "VariableDeclarator"
            )
        }) else {
            return Value::unknown();
        };
        let project = |value: Value<'s>, pattern: Option<NodeId>| {
            let Some(path) = pattern.and_then(|pattern| self.pattern_path(pattern, symbol)) else {
                return Value::unknown();
            };
            self.project(value, &path, scope, depth)
        };
        match tree.kind(writer) {
            "VariableDeclarator" => Value::default(),
            "UpdateExpression" => Value::of(Atom::Primitive(Primitive::Number, Truth::Either)),
            "ForInStatement" => Value::of(Atom::Primitive(Primitive::String, Truth::Either)),
            "ForOfStatement" => {
                let Some(right) = tree.child(writer, "right") else {
                    return Value::unknown();
                };
                let elements = self.project(
                    self.evaluate_in(right, scope, depth + 1),
                    &[Key::Dynamic],
                    scope,
                    depth,
                );
                project(elements, tree.child(writer, "left"))
            }
            _ => {
                let Some(right) = tree.child(writer, "right") else {
                    return Value::unknown();
                };
                match tree.text(writer, "operator") {
                    Some("=") => project(
                        self.evaluate_in(right, scope, depth + 1),
                        tree.child(writer, "left"),
                    ),
                    Some("||=" | "&&=" | "??=") => self.evaluate_in(right, scope, depth + 1),
                    _ => Value::of(Atom::Primitive(Primitive::Other, Truth::Either)),
                }
            }
        }
    }

    /// What reading `path` from `value` gives.
    fn project(
        &self,
        mut value: Value<'s>,
        path: &[Key<'s>],
        scope: &Scope<'s>,
        depth: usize,
    ) -> Value<'s> {
        for &key in path {
            let mut next = Value::default();
            for atom in value.0 {
                next = next.join(self.get(atom, key, scope, depth));
            }
            value = next;
        }
        value
    }

    /// The value a declarator gives `symbol`: its initializer, projected through the pattern
    /// that binds it.
    fn declared_value(
        &self,
        node: NodeId,
        symbol: SymbolId,
        scope: &Scope<'s>,
        depth: usize,
    ) -> Value<'s> {
        let tree = self.tree;
        let declarator = std::iter::once(node)
            .chain(tree.ancestors(node))
            .find(|&outer| tree.kind(outer) == "VariableDeclarator");
        let Some(declarator) = declarator else {
            return Value::unknown();
        };
        let Some(id) = tree.child(declarator, "id") else {
            return Value::unknown();
        };
        let value = match tree.child(declarator, "init") {
            Some(init) => self.evaluate_in(init, scope, depth),
            None => {
                // `for (const x of xs)` binds each element of `xs`; `for (const k in o)` a key.
                let head = tree
                    .parent(declarator)
                    .and_then(|declaration| Some((declaration, tree.parent(declaration)?)));
                match head {
                    Some((declaration, statement))
                        if tree.child(statement, "left") == Some(declaration) =>
                    {
                        match tree.kind(statement) {
                            "ForOfStatement" => match tree.child(statement, "right") {
                                Some(right) => self.project(
                                    self.evaluate_in(right, scope, depth + 1),
                                    &[Key::Dynamic],
                                    scope,
                                    depth,
                                ),
                                None => return Value::unknown(),
                            },
                            "ForInStatement" => {
                                return Value::of(Atom::Primitive(Primitive::String, Truth::Either))
                            }
                            _ => return Value::of(Atom::Undefined),
                        }
                    }
                    _ => return Value::of(Atom::Undefined),
                }
            }
        };
        let Some(path) = self.pattern_path(id, symbol) else {
            return Value::unknown();
        };
        self.project(value, &path, scope, depth)
    }

    /// The keys a pattern reads on its way to the binding of `symbol`; `None` past a default
    /// value, a rest element or a computed key.
    fn pattern_path(&self, pattern: NodeId, symbol: SymbolId) -> Option<Vec<Key<'s>>> {
        let tree = self.tree;
        match tree.kind(pattern) {
            "Identifier" => (self.declares(pattern) == Some(symbol)
                || self.read(pattern) == Some(symbol))
            .then(Vec::new),
            "ObjectPattern" => tree
                .list(pattern, "properties")
                .into_iter()
                .flatten()
                .find_map(|property| {
                    if tree.kind(property) != "Property" {
                        return None;
                    }
                    let name = property_name(tree, property)?;
                    let mut path = self.pattern_path(tree.child(property, "value")?, symbol)?;
                    path.insert(0, Key::Name(name));
                    Some(path)
                }),
            "ArrayPattern" => tree
                .list(pattern, "elements")
                .into_iter()
                .enumerate()
                .find_map(|(index, element)| {
                    let element = element?;
                    if tree.kind(element) == "RestElement" {
                        return None;
                    }
                    let mut path = self.pattern_path(element, symbol)?;
                    path.insert(0, Key::Index(index));
                    Some(path)
                }),
            _ => None,
        }
    }

    /// The framework value `value` is, as a root and the steps applied to it: that value when
    /// every alternative is the same one, that registration when every alternative registers it
    /// (`only ? test.only : test`), a wrapper when some may register and others not or register
    /// differently, nothing when none may. A builder not called yet (`test.each`) registers
    /// nothing when called, so it is not one that may.
    pub(super) fn resolve(&self, value: &Value<'s>) -> Option<(Origin, Vec<Step<'s>>)> {
        let mut first: Option<(Origin, Vec<Step<'s>>)> = None;
        let mut uniform = true;
        // Every alternative registers this when called, as it stands; `None` once one does not.
        let mut same: Option<Option<Registers>> = None;
        let mut registering = false;
        let mut other = false;
        for atom in &value.0 {
            let verdict = match atom {
                Atom::Api(origin, steps) => {
                    match &first {
                        None => first = Some((*origin, steps.clone())),
                        Some((held, held_steps)) => {
                            uniform &= held == origin && held_steps == steps;
                        }
                    }
                    self.verdict(*origin, steps)
                }
                _ => Verdict::Other,
            };
            let registers = match verdict {
                Verdict::Registers(registers) => {
                    registering = true;
                    Some(registers)
                }
                Verdict::Wrapper => {
                    registering = true;
                    None
                }
                Verdict::Namespace => None,
                Verdict::Other => {
                    other = true;
                    None
                }
            };
            same = Some(match same {
                None => registers,
                Some(held) => held.filter(|_| held == registers),
            });
        }
        let (origin, steps) = first?;
        if uniform && !other {
            return Some((origin, steps));
        }
        if let Some(Some(registers)) = same {
            return Some((Origin::Api(registers), Vec::new()));
        }
        registering.then_some((Origin::Wrapper(Reach::Tasks), Vec::new()))
    }

    /// What calling a framework value registers, as the dialect reads its steps.
    fn verdict(&self, origin: Origin, steps: &[Step]) -> Verdict {
        if origin.reach().is_some() {
            return Verdict::Wrapper;
        }
        match self.apply(origin, steps) {
            None => Verdict::Other,
            Some(applied) if applied.pending => Verdict::Other,
            Some(applied) => applied
                .registers
                .map_or(Verdict::Namespace, Verdict::Registers),
        }
    }

    /// Whether the callee chain walk hands the member read at `node` to the value flow: its key
    /// is not a name, or its object is not itself a chain.
    pub(super) fn flows_through(&self, node: NodeId) -> bool {
        let tree = self.tree;
        // A function's own properties are JavaScript's: the receiver decides what they are.
        if matches!(member_name(tree, node), None | Some("name" | "length")) {
            return true;
        }
        let Some(object) = tree.child(node, "object").map(|object| tree.inner(object)) else {
            return true;
        };
        match tree.kind(object) {
            "MemberExpression" | "CallExpression" | "TaggedTemplateExpression" | "MetaProperty" => {
                false
            }
            "Identifier" => self.flows_from(object),
            _ => true,
        }
    }

    /// Whether the identifier `node` reads a binding only the value flow knows exactly: a project
    /// module's namespace, or a declarator the alias pass resolved (its origin there is lossy).
    pub(super) fn flows_from(&self, node: NodeId) -> bool {
        matches!(
            self.parsed.scopes.name_at(self.tree.span(node).start),
            Some(Name::Reference(Some(symbol)))
                if self.namespaces.contains_key(&symbol)
                    || self.aliased.contains(&symbol)
                    || self.imported.contains_key(&symbol)
        )
    }

    /// The callee chain walk's answer at `node` from the value flow, with the `outer` steps the
    /// walk took to reach it (outermost first).
    pub(super) fn flow_root(
        &self,
        node: NodeId,
        mut outer: Vec<Step<'s>>,
    ) -> Option<(Origin, super::Provenance, Vec<Step<'s>>)> {
        let value = self.evaluate(node);
        outer.reverse();
        let (origin, steps) = match self.resolve(&value) {
            Some((origin, mut steps)) => {
                steps.extend(outer);
                (origin, steps)
            }
            // Alternatives that do not settle as a root may once the chain's own steps are
            // applied to each (`api.beforeEach` where `api` is `v` or `{ beforeEach: cases }`).
            None if !outer.is_empty() => self.resolve(&self.stepped(value, &outer))?,
            None => return None,
        };
        let provenance = match origin {
            Origin::Wrapper(_) => super::Provenance::Wrapper,
            Origin::Derived(_) => super::Provenance::Derived,
            _ => super::Provenance::Direct,
        };
        Some((origin, provenance, steps))
    }

    /// `value` after the member reads and argument-less calls of `steps`, root outwards.
    fn stepped(&self, mut value: Value<'s>, steps: &[Step<'s>]) -> Value<'s> {
        let scope = Scope::new();
        for step in steps {
            let mut next = Value::default();
            for atom in value.0 {
                next = next.join(match *step {
                    Step::Member(name) => self.get(atom, Key::Name(name), &scope, 0),
                    Step::Call => self.call(atom, &[], &[], 0),
                });
            }
            value = next;
        }
        value
    }

    /// What `symbol` may hold wherever it is read.
    pub(super) fn value_of(&self, symbol: SymbolId) -> Value<'s> {
        self.flow.clear();
        self.symbol_value(symbol, &Scope::new(), 0)
    }

    /// What applying `steps` to a root of `origin` is, without a node to locate them.
    pub(super) fn chained(&self, origin: Origin, steps: &[Step]) -> Option<Origin> {
        if let Some(reach) = origin.reach() {
            if matches!(origin, Origin::Derived(_)) && steps.is_empty() {
                return Some(origin);
            }
            return Some(Origin::Wrapper(if steps.is_empty() {
                reach
            } else {
                Reach::Tasks
            }));
        }
        let applied = self.apply(origin, steps)?;
        Some(applied.registers.map_or(Origin::Namespace, Origin::Api))
    }
}

/// A truth value as [`Truth`].
fn truth(value: bool) -> Truth {
    if value {
        Truth::Yes
    } else {
        Truth::No
    }
}

/// The value an export summary describes, its steps interned for the process.
fn value_of_summary<'s>(summary: &Summary) -> Value<'s> {
    let mut value = Value::default();
    for (root, steps) in &summary.roots {
        let origin = match *root {
            Root::Api(registers) => Origin::Api(registers),
            Root::Namespace => Origin::Namespace,
            Root::Wrapper(reach) => Origin::Wrapper(reach),
            Root::Derived(registers) => Origin::Derived(registers),
        };
        let steps = steps
            .iter()
            .map(|step| match &**step {
                "()" => Step::Call,
                name => Step::Member(intern(name)),
            })
            .collect();
        value = value.join(Value::of(Atom::Api(origin, steps)));
    }
    if summary.other {
        value = value.join(Value::unknown());
    }
    value
}

/// A member name read from another module's summary, kept for the process: the set of names is
/// the set of member names spelled in the project.
fn intern(name: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Mutex::default).lock().unwrap();
    if let Some(&held) = names.get(name) {
        return held;
    }
    let held: &'static str = Box::leak(name.into());
    names.insert(held);
    held
}

/// The summary an export carries when only its [`Export`] is known.
pub(super) fn summary_of_export(export: Export) -> Summary {
    let root = match export {
        Export::Namespace => Root::Namespace,
        Export::Api { registers, .. } => Root::Api(registers),
        Export::Derived(registers) => Root::Derived(registers),
        Export::Wrapper(reach) => Root::Wrapper(reach),
    };
    Summary {
        roots: vec![(root, Vec::new())],
        other: false,
    }
}

impl<'s> Module<'s> {
    /// What each export of the module holds, from the value flow. The [`Export`] table stays the
    /// authority on which binding a name is: a name a star reaches keeps the flow's summary only
    /// when one project module is its only source and nothing in this module spells it, and a name
    /// the flow does not reach keeps its [`Export`].
    pub(super) fn export_summaries(
        &self,
        modules: &Modules,
        exports: &HashMap<String, Export>,
    ) -> HashMap<String, Summary> {
        let tree = self.tree;
        let dialect = self.setting.dialect;
        let theirs = |source: &str| -> Option<HashMap<String, Summary>> {
            if dialect.is_api_module(source) {
                return None;
            }
            let file = modules
                .sources
                .resolve(self.path, source, self.setting.root)?;
            modules.recorded_summaries(&file, self.setting)
        };
        let mut explicit: HashMap<String, Summary> = HashMap::new();
        let spelled = self.spelled_exports();
        // Each star's source summaries; `None` for a star nothing here can read.
        let mut stars: Vec<Option<HashMap<String, Summary>>> = Vec::new();
        for statement in tree.list(tree.root(), "body").into_iter().flatten() {
            let source = tree
                .child(statement, "source")
                .and_then(|source| tree.text(source, "value"));
            match tree.kind(statement) {
                "ExportNamedDeclaration" => {
                    if tree.text(statement, "exportKind") == Some("type") {
                        continue;
                    }
                    let declaration = tree.child(statement, "declaration");
                    if let Some(source) = source {
                        if let Some(theirs) = theirs(source) {
                            for (local, exported) in super::specifiers(tree, statement) {
                                if let Some(summary) = theirs.get(&export_name(tree, local)) {
                                    explicit.insert(export_name(tree, exported), summary.clone());
                                }
                            }
                        }
                        continue;
                    }
                    if let Some(declaration) =
                        declaration.filter(|&node| tree.kind(node) == "VariableDeclaration")
                    {
                        for declarator in
                            tree.list(declaration, "declarations").into_iter().flatten()
                        {
                            let Some(id) = tree.child(declarator, "id") else {
                                continue;
                            };
                            for symbol in bound_symbols(tree, &self.parsed.scopes, id) {
                                let value = self.value_of(symbol);
                                explicit.insert(
                                    self.parsed.scopes.symbol(symbol).name.clone(),
                                    self.summary_of(&value),
                                );
                            }
                        }
                    }
                    for (local, exported) in super::specifiers(tree, statement) {
                        let value = self.evaluate(local);
                        explicit.insert(export_name(tree, exported), self.summary_of(&value));
                    }
                }
                "ExportDefaultDeclaration" => {
                    let Some(declaration) = tree.child(statement, "declaration") else {
                        continue;
                    };
                    let declaration = tree.inner(declaration);
                    if tree.is_function(declaration) || tree.kind(declaration).starts_with("Class")
                    {
                        continue;
                    }
                    explicit.insert(
                        "default".to_owned(),
                        self.summary_of(&self.evaluate(declaration)),
                    );
                }
                "ExportAllDeclaration"
                    if tree.child(statement, "exported").is_none()
                        && tree.text(statement, "exportKind") != Some("type") =>
                {
                    stars.push(source.and_then(theirs));
                }
                _ => {}
            }
        }
        let mut summaries = HashMap::new();
        if stars.is_empty() {
            summaries.extend(explicit);
        } else if stars.iter().all(Option::is_some) {
            let mut reached: HashMap<&String, Vec<&Summary>> = HashMap::new();
            for theirs in stars.iter().flatten() {
                for (name, summary) in theirs {
                    if name != "default" && !spelled.contains(name) {
                        reached.entry(name).or_default().push(summary);
                    }
                }
            }
            for (name, sources) in reached {
                if let [summary] = sources[..] {
                    summaries.insert(name.clone(), summary.clone());
                }
            }
        }
        for (name, &export) in exports {
            let summary = summaries
                .entry(name.clone())
                .or_insert_with(|| summary_of_export(export));
            // A value built by calling the API (`describe.each(rows)`) stays derived in every
            // importer, as its export says: what it registers depends on the call.
            let built = summary
                .roots
                .iter()
                .any(|(_, steps)| steps.iter().any(|step| &**step == "()"));
            if matches!(export, Export::Derived(_)) && built {
                *summary = summary_of_export(export);
            }
        }
        summaries
    }

    /// What calling the function `function` may register, when the wrapper analysis found it
    /// registering under one of its names.
    fn function_reach(&self, function: NodeId) -> Option<Reach> {
        let own = self
            .tree
            .child(function, "id")
            .and_then(|id| self.declares(id));
        own.into_iter()
            .chain(self.declarator_symbol(function))
            .filter_map(|symbol| self.wrappers.get(&symbol).copied())
            .max()
    }

    /// The summary of a value, for the modules importing it. A function a wrapper analysis found
    /// registering is that wrapper.
    fn summary_of(&self, value: &Value) -> Summary {
        let mut summary = Summary::default();
        for atom in &value.0 {
            match atom {
                Atom::Api(origin, steps) => {
                    let root = match *origin {
                        Origin::Api(registers) => Root::Api(registers),
                        Origin::Namespace => Root::Namespace,
                        Origin::Wrapper(reach) => Root::Wrapper(reach),
                        Origin::Derived(registers) => Root::Derived(registers),
                    };
                    let steps = steps
                        .iter()
                        .map(|step| match step {
                            Step::Call => "()".into(),
                            Step::Member(name) => (*name).into(),
                        })
                        .collect();
                    summary.roots.push((root, steps));
                }
                &Atom::Function(node) => match self.function_reach(node) {
                    Some(reach) => summary.roots.push((Root::Wrapper(reach), Vec::new())),
                    None => summary.other = true,
                },
                _ => summary.other = true,
            }
        }
        summary
    }
}
