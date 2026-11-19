//! What an expression's value may be, from a handful of JavaScript operations over
//! abstract values. Syntax is read in one place, [`Module::lower`]; every rule after it reads
//! operations and values, never node kinds. The framework enters only through [`Atom::Api`],
//! whose members and calls the dialect decides; JavaScript's own property tables decide every
//! other receiver.

use super::{Module, Origin, Step};
use crate::framework::{Holder, Presence};
use crate::inventory::{Export, Modules, Reach, Registers, Root, Summary};
use crate::lang::{
    bound_symbols, export_name, member_name, property_name, Declaration, Field, Name, NodeId,
    SymbolId, Tree,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

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
    /// A function another module exports that registers nothing itself, by what calling it
    /// returns.
    Helper(Box<Summary>),
    /// A project module's namespace object, by the import binding.
    Module(SymbolId),
    /// A framework value: a root the dialect knows and the steps applied to it.
    Api(Origin, Vec<Step<'s>>),
    /// A built-in function or object: truthy, never nullish, never a registration.
    Builtin,
    /// A framework value that registers nothing and whose members the dialect measured
    /// (`v.expect`): truthy, never nullish.
    Framework(Holder),
    /// Anything.
    Unknown,
}

impl Atom<'_> {
    /// A member of a project wrapper, or what calling one returns: any value at all, as
    /// [`Atom::Unknown`] is, while it may still register when called (`wrap.skip ? a : b`).
    fn is_opaque(&self) -> bool {
        matches!(self, Atom::Api(origin, steps) if origin.reach().is_some() && !steps.is_empty())
    }

    /// Whether some value it may be is truthy.
    fn may_be_truthy(&self) -> bool {
        !matches!(
            self,
            Atom::Undefined | Atom::Null | Atom::Primitive(_, Truth::No)
        )
    }

    /// Whether some value it may be is falsy.
    fn may_be_falsy(&self) -> bool {
        matches!(
            self,
            Atom::Undefined
                | Atom::Null
                | Atom::Primitive(_, Truth::No | Truth::Either)
                | Atom::Unknown
        ) || self.is_opaque()
    }

    /// Whether some value it may be is `null` or `undefined`.
    fn may_be_nullish(&self) -> bool {
        matches!(self, Atom::Undefined | Atom::Null | Atom::Unknown) || self.is_opaque()
    }

    /// Whether some value it may be is neither `null` nor `undefined`.
    fn may_be_other_than_nullish(&self) -> bool {
        !matches!(self, Atom::Undefined | Atom::Null)
    }
}

/// The values an expression may evaluate to; empty when it always throws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Value<'s>(Vec<Atom<'s>>);

/// More alternatives than this widen to [`Atom::Unknown`], or to [`Value::exhausted`] when one
/// of them may register.
const WIDEST: usize = 16;
/// Nested evaluations past this depth are [`Value::exhausted`].
const DEEPEST: usize = 48;
/// Steps one evaluation may take, memoised or not, before the rest are [`Value::exhausted`]:
/// depth alone does not bound branching through calls whose scopes are not memoised.
const BUSIEST: usize = 20_000;
/// Containers `holds_within` opens, nested ones included, before the rest may hold
/// anything.
const FULLEST: usize = 1_024;

impl<'s> Value<'s> {
    /// Exactly `atom`.
    fn of(atom: Atom<'s>) -> Self {
        Value(vec![atom])
    }

    /// Any value that registers nothing when called.
    fn unknown() -> Self {
        Value::of(Atom::Unknown)
    }

    /// What a value past an evaluation limit is: any value, which may register when called. A
    /// limit proves nothing absent.
    fn exhausted() -> Self {
        Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), vec![Step::Call]))
    }

    /// Either value: every alternative of both, widened past [`WIDEST`].
    fn join(mut self, other: Value<'s>) -> Self {
        for atom in other.0 {
            if !self.0.contains(&atom) {
                self.0.push(atom);
            }
        }
        if self.0.len() > WIDEST {
            let plain = self.0.iter().all(|atom| {
                matches!(
                    atom,
                    Atom::Undefined
                        | Atom::Null
                        | Atom::Primitive(..)
                        | Atom::Builtin
                        | Atom::Framework(_)
                        | Atom::Unknown
                )
            });
            return if plain {
                Value::unknown()
            } else {
                Value::exhausted()
            };
        }
        self
    }

    /// The alternatives `keep` accepts.
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

    /// Whether some alternative is a framework or project registration value.
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
    /// An optional chain: a `?.` link whose object is nullish makes the whole chain `undefined`.
    Chain(NodeId),
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

// The prototype methods past `Object.prototype`'s, measured on Node 24.21 and 26.0.
/// `Function.prototype`'s methods.
const FUNCTION_MEMBERS: [&str; 3] = ["apply", "bind", "call"];
/// `String.prototype`'s methods.
const STRING_MEMBERS: [&str; 48] = [
    "anchor",
    "at",
    "big",
    "blink",
    "bold",
    "charAt",
    "charCodeAt",
    "codePointAt",
    "concat",
    "endsWith",
    "fixed",
    "fontcolor",
    "fontsize",
    "includes",
    "indexOf",
    "isWellFormed",
    "italics",
    "lastIndexOf",
    "link",
    "localeCompare",
    "match",
    "matchAll",
    "normalize",
    "padEnd",
    "padStart",
    "repeat",
    "replace",
    "replaceAll",
    "search",
    "slice",
    "small",
    "split",
    "startsWith",
    "strike",
    "sub",
    "substr",
    "substring",
    "sup",
    "toLocaleLowerCase",
    "toLocaleUpperCase",
    "toLowerCase",
    "toUpperCase",
    "toWellFormed",
    "trim",
    "trimEnd",
    "trimLeft",
    "trimRight",
    "trimStart",
];
/// `Number.prototype`'s methods; `Boolean.prototype` adds none.
const NUMBER_MEMBERS: [&str; 3] = ["toExponential", "toFixed", "toPrecision"];
/// `Array.prototype`'s methods.
const ARRAY_MEMBERS: [&str; 36] = [
    "at",
    "concat",
    "copyWithin",
    "entries",
    "every",
    "fill",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flat",
    "flatMap",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "keys",
    "lastIndexOf",
    "map",
    "pop",
    "push",
    "reduce",
    "reduceRight",
    "reverse",
    "shift",
    "slice",
    "some",
    "sort",
    "splice",
    "toReversed",
    "toSorted",
    "toSpliced",
    "unshift",
    "values",
    "with",
];
/// The built-in methods that change their receiver's members.
const MUTATORS: [&str; 11] = [
    "copyWithin",
    "fill",
    "pop",
    "push",
    "reverse",
    "shift",
    "sort",
    "splice",
    "unshift",
    "__defineGetter__",
    "__defineSetter__",
];
/// `Object.prototype`'s methods.
const OBJECT_MEMBERS: [&str; 11] = [
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// The key a property name spells: a canonical array index is that index (`'a'['0']`), any other
/// spelling a name (`[x]['00']`).
fn key_named(name: &str) -> Key<'_> {
    match name.parse::<u32>() {
        Ok(index) if index < u32::MAX && index.to_string() == name => Key::Index(index as usize),
        _ => Key::Name(name),
    }
}

/// Whether an object literal's `property` hides what a member read of the literal finds: a
/// spread, a getter or setter, a key with no static name, or `__proto__`, which replaces the
/// literal's prototype so a missing key is not proved absent.
fn opaque_property(tree: &Tree, property: NodeId) -> bool {
    tree.kind(property) != "Property"
        || tree.text(property, "kind") != Some("init")
        || matches!(property_name(tree, property), None | Some("__proto__"))
}

/// What `key` reads on a value whose own properties are `own` and whose prototype's methods are
/// `methods`: a method is a built-in function, anything else is `undefined`.
fn table<'s>(key: Key<'s>, own: &[(&str, Atom<'s>)], methods: &[&str]) -> Value<'s> {
    match key {
        Key::Name(name) => {
            if let Some((_, atom)) = own.iter().find(|(own, _)| *own == name) {
                Value::of(atom.clone())
            } else if name == "__proto__" {
                Value::unknown()
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
    /// Steps taken by the evaluation in progress.
    steps: std::cell::Cell<usize>,
    /// The innermost right side of each assignment-chain link walked; syntax, so never cleared.
    ends: RefCell<HashMap<NodeId, NodeId>>,
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
            // A bigint serializes its value as `null`; its digits tell it from `null`.
            "Literal"
                if matches!(
                    tree.field_of(node, "bigint"),
                    Field::Value(serde_json::Value::String(_))
                ) =>
            {
                let zero = matches!(
                    tree.field_of(node, "bigint"),
                    Field::Value(serde_json::Value::String(digits)) if digits.trim_start_matches(['0', '-']).is_empty()
                );
                Op::Is(Atom::Primitive(Primitive::Other, truth(!zero)))
            }
            "Literal" if matches!(tree.field_of(node, "regex"), Field::Object(_)) => {
                Op::Is(Atom::Builtin)
            }
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
            // A logical assignment is its target's value or, when the operator assigns, the right.
            "AssignmentExpression" => match (child("left"), child("right")) {
                (Some(left), Some(right)) => match tree.text(node, "operator") {
                    Some("&&=") => Op::Choose {
                        test: left,
                        guard: Guard::Truthy,
                        then: Some(right),
                        otherwise: None,
                    },
                    Some("||=") => Op::Choose {
                        test: left,
                        guard: Guard::Truthy,
                        then: None,
                        otherwise: Some(right),
                    },
                    Some("??=") => Op::Choose {
                        test: left,
                        guard: Guard::NotNullish,
                        then: None,
                        otherwise: Some(right),
                    },
                    _ => Op::Unknown,
                },
                _ => Op::Unknown,
            },
            "MemberExpression" => {
                child("object").map_or(Op::Unknown, |object| Op::Get(object, self.key(node)))
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
            "ChainExpression" => child("expression").map_or(Op::Unknown, Op::Chain),
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

    /// The key the member expression `member` reads.
    fn key(&self, member: NodeId) -> Key<'s> {
        let tree = self.tree;
        match member_name(tree, member) {
            // A canonical array index spelled as a string is that index (`'a'['0']`); any other
            // spelling is a name (`[x]['00']`).
            Some(name) => key_named(name),
            None => tree
                .child(member, "property")
                .map(|property| tree.inner(property))
                .and_then(|property| match tree.field_of(property, "value") {
                    Field::Value(serde_json::Value::Number(number)) => {
                        number.as_u64().map(|index| Key::Index(index as usize))
                    }
                    _ => None,
                })
                .unwrap_or(Key::Dynamic),
        }
    }

    /// What `node` may evaluate to.
    pub(super) fn evaluate(&self, node: NodeId) -> Value<'s> {
        // What the module knows of its bindings may have grown since the last evaluation.
        self.flow.clear();
        self.evaluate_in(node, &Scope::new(), 0)
    }

    /// What `node` may evaluate to with `scope`'s parameters bound, `depth` evaluations deep.
    fn evaluate_in(&self, node: NodeId, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let node = self.tree.inner(node);
        #[cfg(test)]
        super::WALK_STEPS.with(|steps| steps.set(steps.get() + 1));
        if self.flow.active.borrow().is_empty() {
            self.flow.steps.set(0);
        }
        self.flow.steps.set(self.flow.steps.get() + 1);
        if depth > DEEPEST || self.flow.steps.get() > BUSIEST {
            return Value::exhausted();
        }
        if self.flow.active.borrow().contains(&node) {
            return Value::unknown();
        }
        let memoised = scope.is_empty();
        if memoised {
            if let Some(value) = self.flow.memo.borrow().get(&node) {
                return value.clone();
            }
        }
        if let Some(value) = self.assignment_chain(node, scope, depth) {
            return value;
        }
        self.flow.active.borrow_mut().push(node);
        let value = self.apply_op(self.lower(node), scope, depth + 1);
        self.flow.active.borrow_mut().pop();
        if memoised {
            self.flow.memo.borrow_mut().insert(node, value.clone());
        }
        value
    }

    /// What a right-nested chain of plain assignments starting at `node` evaluates to (`a = b =
    /// x` is `x`): its innermost right side, evaluated without one evaluation per link so the
    /// chain's length does not count as depth; `None` when `node` does not start one.
    fn assignment_chain(&self, node: NodeId, scope: &Scope<'s>, depth: usize) -> Option<Value<'s>> {
        let end = self.chain_end(node)?;
        // Active, so the evaluation below keeps this one's work allowance.
        self.flow.active.borrow_mut().push(node);
        let value = self.evaluate_in(end, scope, depth + 1);
        self.flow.active.borrow_mut().pop();
        if scope.is_empty() {
            self.flow.memo.borrow_mut().insert(node, value.clone());
        }
        Some(value)
    }

    /// The innermost right side of the chain of two or more plain assignments `node` starts.
    /// Every link's end is kept for the module, so a chain is walked once however many of its
    /// suffixes are evaluated.
    fn chain_end(&self, node: NodeId) -> Option<NodeId> {
        let tree = self.tree;
        let assignment = |node: NodeId| {
            (tree.kind(node) == "AssignmentExpression" && tree.text(node, "operator") == Some("="))
                .then(|| tree.child(node, "right").map(|right| tree.inner(right)))
                .flatten()
        };
        if let Some(&end) = self.flow.ends.borrow().get(&node) {
            return Some(end);
        }
        let mut right = assignment(node)?;
        assignment(right)?;
        let mut links = vec![node];
        let end = loop {
            if let Some(&end) = self.flow.ends.borrow().get(&right) {
                break end;
            }
            match assignment(right) {
                Some(next) => {
                    links.push(right);
                    right = next;
                }
                None => break right,
            }
        };
        let mut ends = self.flow.ends.borrow_mut();
        for link in links {
            ends.insert(link, end);
        }
        Some(end)
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
                } else if awaited.0.iter().all(|atom| matches!(atom, Atom::Api(..))) {
                    // A framework value has no `then`, so awaiting it gives it back.
                    awaited
                } else if awaited.holds_api() {
                    Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), Vec::new()))
                } else {
                    awaited
                        .filter(|atom| !matches!(atom, Atom::Function(_)))
                        .join(Value::unknown())
                }
            }
            Op::Get(object, key) => self.member(object, || evaluate(object), key, scope, depth),
            Op::Call(callee, arguments) => {
                let callee = self
                    .wrapper_body(callee)
                    .unwrap_or_else(|| evaluate(callee));
                self.called(callee, &arguments, scope, depth)
            }
            Op::Chain(expression) => self.optional_chain(expression, scope, depth),
            Op::Read(identifier) => self.read_value(identifier, scope, depth),
        }
    }

    /// What reading `key` of `object`, whose value is `value`, gives.
    fn member(
        &self,
        object: NodeId,
        value: impl FnOnce() -> Value<'s>,
        key: Key<'s>,
        scope: &Scope<'s>,
        depth: usize,
    ) -> Value<'s> {
        if let Some(function) = self.own_members(object) {
            return self.get(Atom::Function(function), key, scope, depth);
        }
        let mut read = Value::default();
        for atom in value().0 {
            read = read.join(self.get(atom, key, scope, depth));
        }
        read
    }

    /// A project wrapper `callee` names, as the body its call returns from: a local function
    /// never reassigned, or an import whose summary records the call. The call stays a wrapper
    /// site, since sites read the callee; only its result is the body's (`(await run()).stderr`).
    fn wrapper_body(&self, callee: NodeId) -> Option<Value<'s>> {
        let tree = self.tree;
        let identifier = tree.inner(callee);
        if tree.kind(identifier) != "Identifier" {
            return None;
        }
        let Some(Name::Reference(Some(symbol))) =
            self.parsed.scopes.name_at(tree.span(identifier).start)
        else {
            return None;
        };
        if let Some(called) = self
            .imported
            .get(&symbol)
            .and_then(|summary| summary.called.as_ref())
        {
            return Some(Value::of(Atom::Helper(called.clone())));
        }
        if !self.wrappers.contains_key(&symbol) || self.reassigned(symbol) {
            return None;
        }
        let function = self.declared_function(symbol)?;
        let returned = self.call(Atom::Function(function), &[], &[], 0);
        self.registers_nothing(&returned, 0)
            .then(|| Value::of(Atom::Function(function)))
    }

    /// Whether nothing `value` may hold can register: primitives, `undefined`, `null`, built-ins,
    /// measured framework values and containers of those. A wrapper's result that may hold a
    /// function stays opaque, since a function it returns may register (`make().run()`).
    fn registers_nothing(&self, value: &Value<'s>, depth: usize) -> bool {
        value.0.iter().all(|atom| match atom {
            Atom::Undefined
            | Atom::Null
            | Atom::Primitive(..)
            | Atom::Builtin
            | Atom::Framework(_) => true,
            Atom::Object(_) | Atom::Array(_) => {
                depth < 3
                    && self.registers_nothing(
                        &self.get(atom.clone(), Key::Dynamic, &Scope::new(), 0),
                        depth + 1,
                    )
            }
            _ => false,
        })
    }

    /// What calling `callee` with `arguments` returns.
    fn called(
        &self,
        callee: Value<'s>,
        arguments: &[NodeId],
        scope: &Scope<'s>,
        depth: usize,
    ) -> Value<'s> {
        let handed: Vec<Value<'s>> = arguments
            .iter()
            .map(|&argument| self.evaluate_in(argument, scope, depth))
            .collect();
        // An argument that throws stops the call before it happens.
        if handed.iter().any(Value::throws) {
            return Value::default();
        }
        let mut value = Value::default();
        for atom in callee.0 {
            value = value.join(self.call(atom, arguments, &handed, depth));
        }
        value
    }

    /// What the optional chain `expression` evaluates to: its links from the innermost out, each
    /// `?.` link continuing with the object's alternatives that are not nullish, and `undefined`
    /// when one may be.
    fn optional_chain(&self, expression: NodeId, scope: &Scope<'s>, depth: usize) -> Value<'s> {
        let tree = self.tree;
        let mut links = Vec::new();
        let mut node = tree.inner(expression);
        loop {
            let next = match tree.kind(node) {
                "MemberExpression" if self.root_origin(node).is_none() => {
                    tree.child(node, "object")
                }
                "CallExpression" => tree.child(node, "callee"),
                _ => None,
            };
            let Some(next) = next else { break };
            links.push(node);
            node = tree.inner(next);
        }
        let mut value = self.evaluate_in(node, scope, depth);
        let mut skipped = false;
        let mut object = node;
        for &link in links.iter().rev() {
            if tree.flag(link, "optional") {
                skipped |= value.0.iter().any(Atom::may_be_nullish);
                value = value.filter(Atom::may_be_other_than_nullish);
            }
            value = if tree.kind(link) == "MemberExpression" {
                self.member(object, || value, self.key(link), scope, depth)
            } else {
                let arguments: Vec<NodeId> =
                    tree.list(link, "arguments").into_iter().flatten().collect();
                self.called(value, &arguments, scope, depth)
            };
            object = link;
        }
        if skipped {
            value = value.join(Value::of(Atom::Undefined));
        }
        value
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
            Atom::Primitive(Primitive::String, _) => match key {
                Key::Index(_) => Value::of(any_string).join(Value::of(Atom::Undefined)),
                _ => table(key, &[("length", any_number)], &STRING_MEMBERS),
            },
            Atom::Primitive(Primitive::Number, _) => table(key, &[], &NUMBER_MEMBERS),
            Atom::Primitive(Primitive::Boolean, _) => table(key, &[], &[]),
            // Another module may write a member of its function, which is not seen here.
            Atom::Primitive(Primitive::Other, _)
            | Atom::Builtin
            | Atom::Unknown
            | Atom::Helper(_) => match atom {
                Atom::Unknown | Atom::Helper(_) => Value::unknown(),
                _ => Value::of(Atom::Builtin).join(Value::of(Atom::Undefined)),
            },
            Atom::Framework(holder) => match key {
                Key::Name("name" | "length") if holder != Holder::Namespace => {
                    Value::of(if matches!(key, Key::Name("name")) {
                        any_string
                    } else {
                        any_number
                    })
                }
                Key::Name(name) => self.framework_member(Some(holder), name),
                _ => Value::unknown(),
            },
            // Accessors that throw in strict code, and a `prototype` only some functions have.
            Atom::Function(_) if matches!(key, Key::Name("arguments" | "caller" | "prototype")) => {
                Value::unknown()
            }
            // A member written to the function through its binding (`f.flag = true`).
            Atom::Function(node) if self.function_members_written(node) => Value::unknown(),
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
                    Key::Name(_) => table(key, &[], &ARRAY_MEMBERS),
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
                let opaque = properties
                    .iter()
                    .any(|&property| opaque_property(tree, property));
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
                    Some(summary) => self.value_of_summary(summary),
                    // The module's other exports register nothing when called.
                    None => Value::of(Atom::Builtin).join(Value::of(Atom::Undefined)),
                },
                _ => Value::unknown(),
            },
            Atom::Api(origin, mut steps) => match key {
                // A function's own properties are JavaScript's; only the module namespace itself
                // is an object without them. What a wrapper returns or holds may be any value.
                Key::Name("name" | "length")
                    if (origin.reach().is_none() || steps.is_empty())
                        && !(origin == Origin::Namespace && steps.is_empty()) =>
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
                    let holder = self
                        .apply(origin, &steps)
                        .and_then(|applied| applied.holder);
                    steps.push(Step::Member(name));
                    match self.apply(origin, &steps) {
                        Some(applied) if applied.unresolved => {
                            Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), Vec::new()))
                        }
                        Some(_) => Value::of(Atom::Api(origin, steps)),
                        // A member that is not the API's is what the dialect measured: `undefined`
                        // when the value lacks it (`v.skip`, `test.expect`), else a value that
                        // registers nothing (`test.fn`, `v.expect`).
                        None => self.framework_member(holder, name),
                    }
                }
                _ => Value::unknown(),
            },
        }
    }

    /// The member `name` of a framework value of kind `holder` that is not a registration one.
    fn framework_member(&self, holder: Option<Holder>, name: &str) -> Value<'s> {
        let dialect = self.setting.dialect;
        match holder.map(|holder| (holder, dialect.presence(holder, name, self.setting.major))) {
            Some((_, Presence::Absent)) => Value::of(Atom::Undefined),
            Some((Holder::Namespace, Presence::Present)) if dialect.assertion(name).is_some() => {
                Value::of(Atom::Framework(Holder::Expect))
            }
            Some((_, Presence::Present)) => Value::of(Atom::Builtin),
            Some((_, Presence::Unresolved)) => {
                Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), Vec::new()))
            }
            None => Value::unknown(),
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
            // A registration function a project function returns is derived from that function:
            // what the call registers is the function's to prove (`id(test)`, `t => t.runIf(c)`).
            // A builder not called yet (`test.each`) registers nothing, through a helper or not.
            Atom::Function(function) => {
                let mut value = self.returned(function, arguments, handed, depth);
                for atom in &mut value.0 {
                    if let Atom::Api(origin @ Origin::Api(_), steps) = atom {
                        if let Verdict::Registers(registers) = self.verdict(*origin, steps) {
                            *atom = Atom::Api(Origin::Derived(registers), Vec::new());
                        }
                    }
                }
                value
            }
            // What another module's helper returns, derived there as a local one's is here.
            // One whose result is not known may return a registration function handed to it.
            Atom::Helper(returns) if handed_api && returns.other => {
                self.value_of_summary(&returns).join(Value::of(Atom::Api(
                    Origin::Wrapper(Reach::Tasks),
                    Vec::new(),
                )))
            }
            Atom::Helper(returns) => self.value_of_summary(&returns),
            Atom::Builtin | Atom::Framework(_) | Atom::Module(_) | Atom::Unknown if handed_api => {
                // What a call handed a registration function returns may register.
                Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), Vec::new()))
            }
            Atom::Builtin | Atom::Framework(_) | Atom::Module(_) => {
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
            let value = self.evaluate_in(body, &scope, depth);
            return self.resumed(function, value, &scope);
        }
        // A body whose last statement returns or throws never runs off its end.
        let ends = tree
            .list(body, "body")
            .last()
            .copied()
            .flatten()
            .is_some_and(|last| matches!(tree.kind(last), "ReturnStatement" | "ThrowStatement"));
        let mut value = if ends {
            Value::default()
        } else {
            Value::of(Atom::Undefined)
        };
        // A generator's iterator hands out what it yields as well as what it returns.
        let generator = tree.flag(function, "generator");
        for node in tree.descendants(body) {
            let kind = tree.kind(node);
            if kind != "ReturnStatement" && !(generator && kind == "YieldExpression") {
                continue;
            }
            let nested = tree
                .ancestors(node)
                .take_while(|&outer| outer != function)
                .any(|outer| tree.is_function(outer));
            // A `return` past an earlier one in its block never runs.
            if nested || self.unreachable(node) {
                continue;
            }
            value = value.join(match tree.child(node, "argument") {
                // A delegated iterator hands out what this one does not see, which may register
                // unless nothing in it does.
                Some(argument) if tree.flag(node, "delegate") => {
                    let delegated = self.evaluate_in(argument, &scope, depth);
                    if self.registers_nothing(&delegated, 0) {
                        Value::unknown()
                    } else {
                        Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), vec![Step::Call]))
                    }
                }
                Some(argument) => self.evaluate_in(argument, &scope, depth),
                None => Value::of(Atom::Undefined),
            });
        }
        self.resumed(function, value, &scope)
    }

    /// What calling `function` gives when its body returns `value`: that value, or for an async
    /// function or a generator the promise or iterator holding it. Such an object is truthy and
    /// has only built-in members; when what it holds may register, it is any value that may, and
    /// when it holds a value nothing here knows, it may be that value, as the same body without
    /// `async` would return. What it holds is read with `scope`, the body's bound parameters.
    fn resumed(&self, function: NodeId, value: Value<'s>, scope: &Scope<'s>) -> Value<'s> {
        let tree = self.tree;
        if !tree.flag(function, "async") && !tree.flag(function, "generator") {
            return value;
        }
        if self.registers_nothing(&value, 0) {
            Value::of(Atom::Builtin)
        } else if self.may_hold_registration(&value, scope, 0) {
            Value::of(Atom::Api(Origin::Wrapper(Reach::Tasks), vec![Step::Call]))
        } else {
            Value::of(Atom::Builtin).join(Value::unknown())
        }
    }

    /// Whether `value` holds something other than an unknown value that may register: a
    /// registration value, a function, a module, or a container that is not a
    /// [plain literal](Module::plain_literal) or has a member that may be one, read with `scope`.
    /// A container deeper than [`Module::registers_nothing`] opens may.
    fn may_hold_registration(&self, value: &Value<'s>, scope: &Scope<'s>, depth: usize) -> bool {
        value.0.iter().any(|atom| match atom {
            Atom::Undefined
            | Atom::Null
            | Atom::Primitive(..)
            | Atom::Builtin
            | Atom::Framework(_)
            | Atom::Unknown => false,
            Atom::Object(_) | Atom::Array(_) => {
                depth >= 3
                    || !self.plain_literal(atom)
                    || self.may_hold_registration(
                        &self.get(atom.clone(), Key::Dynamic, scope, 0),
                        scope,
                        depth + 1,
                    )
            }
            _ => true,
        })
    }

    /// Whether the container literal `atom` holds only data a member read sees: elements without
    /// a spread, or properties that are plain `key: value` pairs a member read can see
    /// ([`opaque_property`]) with no computed key or method.
    fn plain_literal(&self, atom: &Atom<'s>) -> bool {
        let tree = self.tree;
        match *atom {
            Atom::Array(node) => tree
                .list(node, "elements")
                .into_iter()
                .flatten()
                .all(|element| tree.kind(element) != "SpreadElement"),
            Atom::Object(node) => tree
                .list(node, "properties")
                .into_iter()
                .flatten()
                .all(|part| {
                    !opaque_property(tree, part)
                        && !tree.flag(part, "method")
                        && !tree.flag(part, "computed")
                }),
            _ => false,
        }
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
        // A parameter holds what it was handed and whatever the body writes to it.
        if let Some(value) = scope.get(&symbol) {
            return self.with_writes(value.clone(), symbol, scope, depth);
        }
        if self.namespaces.contains_key(&symbol) {
            return Value::of(Atom::Module(symbol));
        }
        if let Some(summary) = self.imported.get(&symbol) {
            return self.value_of_summary(summary);
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
        let value = if self.redeclared.contains(&symbol) {
            self.redeclared_value(symbol, scope, depth)
        } else {
            match scopes.symbol(symbol).declaration.clone() {
                Declaration::Function(node) => Value::of(Atom::Function(node)),
                Declaration::Class(_) => Value::of(Atom::Builtin),
                Declaration::Variable(node) => self.declared_value(node, symbol, scope, depth),
                _ => Value::unknown(),
            }
        };
        self.with_writes(value, symbol, scope, depth)
    }

    /// `value` joined with every value written to `symbol`, and anything at all in a module that
    /// runs `eval`.
    fn with_writes(
        &self,
        mut value: Value<'s>,
        symbol: SymbolId,
        scope: &Scope<'s>,
        depth: usize,
    ) -> Value<'s> {
        for reference in crate::lang::proof::writes(&self.parsed.scopes, symbol) {
            value = value.join(match reference.node {
                Some(target) => self.written_value(target, symbol, scope, depth),
                None => Value::unknown(),
            });
        }
        if self.evaluates {
            value = value.join(Value::unknown());
        }
        self.unwritten(value, symbol)
    }

    /// `value` read through `symbol`: a container literal whose members may be written through
    /// it is any value, one that may register when it holds a framework value.
    fn unwritten(&self, value: Value<'s>, symbol: SymbolId) -> Value<'s> {
        let containers = |value: &Value<'s>| {
            value
                .0
                .iter()
                .any(|atom| matches!(atom, Atom::Object(_) | Atom::Array(_)))
        };
        if !containers(&value) || !self.members_written(symbol, false, 0) {
            return value;
        }
        let mut read = Value::default();
        for atom in value.0 {
            read = read.join(match atom {
                Atom::Object(_) | Atom::Array(_) if self.holds_within(&atom) => Value::exhausted(),
                Atom::Object(_) | Atom::Array(_) => Value::unknown(),
                atom => Value::of(atom),
            });
        }
        read
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
                    path.insert(0, key_named(name));
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
            Some(applied) if applied.unresolved => Verdict::Wrapper,
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
            "Identifier" => self.flows_from(object) || self.own_members(object).is_some(),
            _ => true,
        }
    }

    /// The function a local wrapper binding read at `identifier` declares, when every member of
    /// it is JavaScript's own: the binding is never reassigned or exported and nothing may write a
    /// member through it ([`Module::members_written`]). A member a function lacks is then
    /// `undefined` (`wrap.skip`), as it is for any other local function; a write through another
    /// binding (`const s = wrap; s.t = test`) is not seen.
    pub(super) fn own_members(&self, identifier: NodeId) -> Option<NodeId> {
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        let identifier = tree.inner(identifier);
        if tree.kind(identifier) != "Identifier" {
            return None;
        }
        let Some(Name::Reference(Some(symbol))) = scopes.name_at(tree.span(identifier).start)
        else {
            return None;
        };
        if !self.wrappers.contains_key(&symbol) || self.reassigned(symbol) {
            return None;
        }
        let function = self.declared_function(symbol)?;
        let exported = tree
            .ancestors(function)
            .any(|outer| tree.kind(outer).starts_with("Export"));
        (!exported && !self.members_written(symbol, true, 0)).then_some(function)
    }

    /// Whether a member of the function `function` may be written through a binding of it.
    fn function_members_written(&self, function: NodeId) -> bool {
        let tree = self.tree;
        let own = (tree.kind(function) == "FunctionDeclaration")
            .then(|| tree.child(function, "id"))
            .flatten()
            .and_then(|id| self.declares(id));
        own.into_iter()
            .chain(self.declarator_symbol(function))
            .any(|symbol| self.members_written(symbol, true, 0))
    }

    /// The function `symbol` is declared as: a declaration, or a declarator initialized with one.
    fn declared_function(&self, symbol: SymbolId) -> Option<NodeId> {
        let tree = self.tree;
        match self.parsed.scopes.symbol(symbol).declaration {
            Declaration::Function(node) => Some(node),
            Declaration::Variable(node) => {
                let declarator = std::iter::once(node)
                    .chain(tree.ancestors(node))
                    .find(|&outer| tree.kind(outer) == "VariableDeclarator")?;
                let init = tree.inner(tree.child(declarator, "init")?);
                tree.is_function(init).then_some(init)
            }
            _ => None,
        }
    }

    /// Whether a member of what `symbol` holds may be written through it: through any of its
    /// references ([`Self::value_written`]). `shallow` asks about its own members only, the only
    /// ones a function's are read as.
    fn members_written(&self, symbol: SymbolId, shallow: bool, depth: usize) -> bool {
        self.parsed
            .scopes
            .symbol(symbol)
            .references
            .iter()
            .any(|reference| {
                reference
                    .node
                    .is_none_or(|node| self.value_written(node, shallow, depth))
            })
    }

    /// Whether a member of the value `node` evaluates to may be written where the value goes: an
    /// export of it, a write or `delete` of a member, a method that changes its receiver, a copy to a
    /// binding whose members may be written, a function's result or a container literal whose
    /// members may be written, a loop or generator it is handed to, or a call whose receiving
    /// parameter may. A choice, `await` or sequence passes the value on unchanged. Only a call of
    /// a local function that is never reassigned is followed, to a parameter it binds by name,
    /// and a body reading `arguments` may write anything. With `shallow`, a member's value handed
    /// on, or a write past the value's own members, changes none of them.
    fn value_written(&self, node: NodeId, shallow: bool, depth: usize) -> bool {
        const DEEPEST_HANDOFF: usize = 4;
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        let deeper = |symbol: SymbolId| {
            depth >= DEEPEST_HANDOFF || self.members_written(symbol, shallow, depth + 1)
        };
        // The member chain the value roots (`o.inner.flag`), through what passes it on.
        let mut top = node;
        let mut members = 0;
        let parent = loop {
            let Some(parent) = tree.outer_parent(top) else {
                return false;
            };
            let at =
                |key: &str| tree.child(parent, key).map(|child| tree.inner(child)) == Some(top);
            match tree.kind(parent) {
                "MemberExpression" if at("object") => members += 1,
                "ConditionalExpression" if !at("test") => {}
                "LogicalExpression" | "AwaitExpression" | "ChainExpression" => {}
                "SequenceExpression"
                    if tree
                        .list(parent, "expressions")
                        .last()
                        .copied()
                        .flatten()
                        .map(|last| tree.inner(last))
                        == Some(top) => {}
                _ => break parent,
            }
            top = parent;
        };
        let at = |key: &str| tree.child(parent, key).map(|child| tree.inner(child)) == Some(top);
        let handed = !shallow || members == 0;
        let written = members > 0 && (!shallow || members == 1);
        match tree.kind(parent) {
            // Another module may write a binding this one exports by name (`export { o }`); an
            // alias it is copied to is not followed there.
            kind if kind.starts_with("Export") => depth == 0 && top == node,
            // Writing the binding itself is a reassignment, which the value flow reads.
            "AssignmentExpression" if at("left") => written,
            "AssignmentExpression" => {
                handed
                    && tree
                        .child(parent, "left")
                        .and_then(|left| self.read(left))
                        .is_none_or(deeper)
            }
            "UpdateExpression" => written,
            "UnaryExpression" => written && tree.text(parent, "operator") == Some("delete"),
            kind if kind.ends_with("Pattern") => written,
            "VariableDeclarator" if at("init") => {
                handed
                    && tree
                        .child(parent, "id")
                        .is_none_or(|id| bound_symbols(tree, scopes, id).into_iter().any(deeper))
            }
            // A method that changes its receiver (`list.push(test)`).
            "CallExpression" | "NewExpression" if at("callee") => {
                written && member_name(tree, top).is_some_and(|name| MUTATORS.contains(&name))
            }
            "CallExpression" | "NewExpression" => {
                handed && self.received_written(parent, top, shallow, depth)
            }
            "ReturnStatement" => {
                handed
                    && tree
                        .ancestors(parent)
                        .find(|&outer| tree.is_function(outer))
                        .is_none_or(|function| self.result_written(function, shallow, depth))
            }
            "ArrowFunctionExpression" if at("body") => {
                handed && self.result_written(parent, shallow, depth)
            }
            // A container holding it, or copying its members, is where it goes next.
            "ArrayExpression" => {
                handed
                    && (depth >= DEEPEST_HANDOFF || self.value_written(parent, shallow, depth + 1))
            }
            "Property" | "SpreadElement" if at("value") || at("argument") => {
                handed
                    && (depth >= DEEPEST_HANDOFF
                        || tree.outer_parent(parent).is_none_or(|container| {
                            !matches!(tree.kind(container), "ObjectExpression" | "ArrayExpression")
                                || self.value_written(container, shallow, depth + 1)
                        }))
            }
            "YieldExpression" | "ForOfStatement" | "ForInStatement" => handed,
            _ => false,
        }
    }

    /// Whether a member of what `function` returns may be written where a call of it goes. A
    /// function read other than as a callee may be called where that is not seen.
    fn result_written(&self, function: NodeId, shallow: bool, depth: usize) -> bool {
        const DEEPEST_HANDOFF: usize = 4;
        let tree = self.tree;
        if depth >= DEEPEST_HANDOFF {
            return true;
        }
        let own = (tree.kind(function) == "FunctionDeclaration")
            .then(|| tree.child(function, "id"))
            .flatten()
            .and_then(|id| self.declares(id));
        let bindings: Vec<SymbolId> = own
            .into_iter()
            .chain(self.declarator_symbol(function))
            .collect();
        bindings.is_empty()
            || bindings.into_iter().any(|binding| {
                self.reassigned(binding)
                    || self
                        .parsed
                        .scopes
                        .symbol(binding)
                        .references
                        .iter()
                        .any(|reference| {
                            let Some(node) = reference.node else {
                                return true;
                            };
                            match tree.outer_parent(node) {
                                Some(call)
                                    if tree.kind(call) == "CallExpression"
                                        && tree
                                            .child(call, "callee")
                                            .map(|callee| tree.inner(callee))
                                            == Some(node) =>
                                {
                                    self.value_written(call, shallow, depth + 1)
                                }
                                _ => true,
                            }
                        })
            })
    }

    /// Whether the call `call` handing it `argument` may write a member of that argument.
    fn received_written(
        &self,
        call: NodeId,
        argument: NodeId,
        shallow: bool,
        depth: usize,
    ) -> bool {
        const DEEPEST_HANDOFF: usize = 4;
        let tree = self.tree;
        let scopes = &self.parsed.scopes;
        if depth >= DEEPEST_HANDOFF {
            return true;
        }
        let Some(callee) = tree.child(call, "callee").map(|callee| tree.inner(callee)) else {
            return true;
        };
        let Some(Name::Reference(Some(callee))) = (tree.kind(callee) == "Identifier")
            .then(|| scopes.name_at(tree.span(callee).start))
            .flatten()
        else {
            return true;
        };
        if self.reassigned(callee) {
            return true;
        }
        let Some(function) = self.declared_function(callee) else {
            return true;
        };
        let reads_arguments = tree.descendants(function).any(|node| {
            tree.kind(node) == "Identifier" && tree.text(node, "name") == Some("arguments")
        });
        let arguments: Vec<NodeId> = tree.list(call, "arguments").into_iter().flatten().collect();
        let Some(index) = arguments
            .iter()
            .position(|&each| tree.inner(each) == argument)
        else {
            return true;
        };
        let spread = arguments[..index]
            .iter()
            .any(|&each| tree.kind(each) == "SpreadElement");
        let parameter = tree.list(function, "params").get(index).copied().flatten();
        match parameter {
            _ if reads_arguments || spread => true,
            None => false,
            Some(parameter) if tree.kind(parameter) == "Identifier" => self
                .declares(parameter)
                .is_none_or(|parameter| self.members_written(parameter, shallow, depth + 1)),
            Some(_) => true,
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
        if let Some(applied) = super::wrapper_applied(origin, steps) {
            return Some(applied);
        }
        let applied = self.apply(origin, steps)?;
        if applied.unresolved {
            return Some(Origin::Wrapper(Reach::Tasks));
        }
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

/// The member names `summaries` spell in their steps, nested summaries included.
pub(super) fn step_names<'a>(summaries: impl Iterator<Item = &'a Summary>) -> HashSet<Box<str>> {
    let mut names = HashSet::new();
    let mut pending: Vec<&Summary> = summaries.collect();
    while let Some(summary) = pending.pop() {
        for (_, steps) in &summary.roots {
            names.extend(steps.iter().filter(|step| &***step != "()").cloned());
        }
        pending.extend(summary.returns.as_deref());
        pending.extend(summary.called.as_deref());
    }
    names
}

impl<'s> Module<'s> {
    /// The value an export summary describes, its member names borrowed from [`Module::names`].
    /// A root spelling a name the module did not collect is unknown.
    fn value_of_summary(&self, summary: &Summary) -> Value<'s> {
        let names: &'s std::cell::OnceCell<HashSet<Box<str>>> = self.names;
        let member = |name: &str| names.get()?.get(name).map(|name| &**name);
        let mut value = Value::default();
        for (root, steps) in &summary.roots {
            let origin = match *root {
                Root::Api(registers) => Origin::Api(registers),
                Root::Namespace => Origin::Namespace,
                Root::Wrapper(reach) => Origin::Wrapper(reach),
                Root::Derived(registers) => Origin::Derived(registers),
            };
            let steps: Option<Vec<Step<'s>>> = steps
                .iter()
                .map(|step| match &**step {
                    "()" => Some(Step::Call),
                    name => member(name).map(Step::Member),
                })
                .collect();
            value = value.join(match steps {
                Some(steps) => Value::of(Atom::Api(origin, steps)),
                None => Value::unknown(),
            });
        }
        if let Some(returns) = &summary.returns {
            value = value.join(Value::of(Atom::Helper(returns.clone())));
        }
        if summary.other {
            value = value.join(Value::unknown());
        }
        value
    }
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
        ..Summary::default()
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
        let theirs = |source: &str| -> Option<&HashMap<String, Summary>> {
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
        let mut stars: Vec<Option<&HashMap<String, Summary>>> = Vec::new();
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
                    if let Some(function) =
                        declaration.filter(|&node| tree.kind(node) == "FunctionDeclaration")
                    {
                        if let Some(symbol) =
                            tree.child(function, "id").and_then(|id| self.declares(id))
                        {
                            explicit.insert(
                                self.parsed.scopes.symbol(symbol).name.clone(),
                                self.summary_of(&self.value_of(symbol)),
                            );
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
                    if tree.kind(declaration).starts_with("Class") {
                        continue;
                    }
                    // An anonymous function has no symbol the wrapper analysis names, so only one
                    // its export does not already make a wrapper is a helper.
                    if tree.is_function(declaration) {
                        if exports.contains_key("default") {
                            continue;
                        }
                        explicit.insert(
                            "default".to_owned(),
                            self.summary_of(&Value::of(Atom::Function(declaration))),
                        );
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
        // A name this module spells is its own whatever its stars reach. Beside a star it also
        // keeps what its export reads, since the spelling may be erased (`export declare const`).
        let beside_stars: HashSet<String> = if stars.is_empty() {
            HashSet::new()
        } else {
            explicit.keys().cloned().collect()
        };
        let mut summaries = explicit;
        if !stars.is_empty() && stars.iter().all(Option::is_some) {
            let mut reached: HashMap<&String, Vec<&Summary>> = HashMap::new();
            for theirs in stars.iter().flatten() {
                for (name, summary) in theirs.iter() {
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
            let table = summary_of_export(export);
            let summary = summaries
                .entry(name.clone())
                .or_insert_with(|| table.clone());
            if beside_stars.contains(name) {
                for root in table.roots {
                    if !summary.roots.contains(&root) {
                        summary.roots.push(root);
                    }
                }
            }
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
    fn summary_of(&self, value: &Value<'s>) -> Summary {
        self.summary_at(value, true)
    }

    /// [`Self::summary_of`]; with `call`, a lone function carries what calling it returns, its
    /// parameters unknown: as its value when it registers nothing, beside its wrapper root when
    /// it does.
    fn summary_at(&self, value: &Value<'s>, call: bool) -> Summary {
        if let [Atom::Function(function)] = &value.0[..] {
            if call {
                let function = *function;
                let parameters = self.tree.list(function, "params").len();
                let unknown = vec![Value::unknown(); parameters];
                let returned_value = self.call(Atom::Function(function), &[], &unknown, 0);
                let returned = Some(Box::new(self.summary_at(&returned_value, false)));
                return match self.function_reach(function) {
                    None => Summary {
                        returns: returned,
                        ..Summary::default()
                    },
                    Some(reach) => Summary {
                        roots: vec![(Root::Wrapper(reach), Vec::new())],
                        called: returned.filter(|_| self.registers_nothing(&returned_value, 0)),
                        ..Summary::default()
                    },
                };
            }
        }
        if let [Atom::Helper(returns)] = &value.0[..] {
            if call {
                return Summary {
                    returns: Some(returns.clone()),
                    ..Summary::default()
                };
            }
        }
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
                // A returned container holding a registration function (`{ it: test }`) is not
                // summarised member by member: what is read from it may register.
                Atom::Object(_) | Atom::Array(_) if !call && self.holds_within(atom) => {
                    summary
                        .roots
                        .push((Root::Wrapper(Reach::Tasks), Vec::new()));
                    summary.other = true;
                }
                _ => summary.other = true,
            }
        }
        summary
    }

    /// Whether a container literal holds a framework value, in a member or a nested container.
    /// Containers are opened one at a time, so nesting depth costs no stack; past [`FULLEST`]
    /// opened, what is left may hold anything.
    fn holds_within(&self, atom: &Atom<'s>) -> bool {
        let mut pending = vec![atom.clone()];
        let mut opened = 0;
        while let Some(atom) = pending.pop() {
            if !matches!(atom, Atom::Object(_) | Atom::Array(_)) {
                continue;
            }
            opened += 1;
            if opened > FULLEST {
                return true;
            }
            let members = self.get(atom, Key::Dynamic, &Scope::new(), 0);
            if members.holds_api() {
                return true;
            }
            pending.extend(members.0);
        }
        false
    }
}
