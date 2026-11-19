//! A value may be a registration function in one branch and not in another; a call of it
//! registers exactly when some value it may evaluate to is one.
//!
//! The test holds no list of which framework functions register. An atom is any member chain of
//! what the prelude binds, and whether calling it registers is the analyzer's own verdict on a
//! direct call of it. Everything above the atoms is JavaScript: which operand an expression may
//! evaluate to, which values are always truthy, and where a value can be bound, exported and
//! imported. What a framework chain is when it does not register (a function, `undefined`) is
//! [`Framework`], measured on Vitest; another framework needs its own table.

use super::{inventory_of, sites};
use crate::testing::property;
use proptest::prelude::*;
use std::collections::HashMap;

/// What the test file and the module both bind before the generated code.
const PRELUDE: &str = "import { test, describe } from 'vitest'\n\
     import * as v from 'vitest'\n\
     const noop = () => {}\n\
     const on = process.env.X !== 'true'\n\
     const wrap = (name: string, fn: () => void) => test(name, fn)\n\
     const identity = <T>(value: T): T => value\n";

/// The roots an atom starts from; the last two are local functions, truthy by construction.
const ROOTS: [&str; 5] = ["test", "describe", "v", "noop", "wrap"];

/// Member names an atom may step through, besides generated ones. A function's own `name` and
/// `length` are left to [`Form::Scalar`], where JavaScript decides them.
const MEMBERS: [&str; 9] = [
    "skip",
    "only",
    "each",
    "extend",
    "for",
    "concurrent",
    "expect",
    "test",
    "todo",
];

/// Names that cannot be generated: the prelude's bindings, keywords, and properties every
/// function has.
const TAKEN: [&str; 21] = [
    "test",
    "describe",
    "v",
    "noop",
    "on",
    "wrap",
    "identity",
    "do",
    "if",
    "in",
    "of",
    "for",
    "let",
    "new",
    "var",
    "name",
    "length",
    "call",
    "apply",
    "bind",
    "prototype",
];

/// Members every function inherits that a generated name may spell.
const INHERITED: [&str; 3] = ["toString", "valueOf", "caller"];

/// One step of a member chain: `.name` or `.name(arguments)`.
#[derive(Clone, Debug)]
struct Step {
    member: String,
    arguments: Option<&'static str>,
}

/// A member chain of a prelude binding; the analyzer decides whether calling it registers.
#[derive(Clone, Debug)]
struct Atom {
    root: &'static str,
    steps: Vec<Step>,
}

impl Atom {
    /// The chain as the module spells it.
    fn source(&self) -> String {
        let mut text = self.root.to_owned();
        for step in &self.steps {
            text.push('.');
            text.push_str(&step.member);
            if let Some(arguments) = step.arguments {
                text.push_str(arguments);
            }
        }
        text
    }

    /// What the atom is, when it does not register. A bare root is a function, or for `v` a
    /// namespace object. A member a local function does not have is `undefined`, so calling it
    /// or reading through it throws. A framework chain of [`MEMBERS`] is what [`Framework`]
    /// measured; one with a call or another member is not known.
    fn plain_value(&self) -> Value {
        let local = matches!(self.root, "noop" | "wrap");
        let Some((first, rest)) = self.steps.split_first() else {
            return Value {
                function: self.root != "v",
                object: self.root == "v",
                ..Value::default()
            };
        };
        if !local {
            return Framework::value(self);
        }
        if INHERITED.contains(&first.member.as_str()) {
            return Value {
                unknown: true,
                ..Value::default()
            };
        }
        if first.arguments.is_some() || !rest.is_empty() {
            return Value::default();
        }
        Value {
            nullish: true,
            ..Value::default()
        }
    }
}

/// What a chain of [`MEMBERS`] from a framework root is. Measured over all 273 such chains of up
/// to two steps (`typeof`, or whether reading throws) on Vitest 3.2.7, 4.1.12 and 5.1.0, which
/// agree on every one.
#[derive(Clone, Copy)]
enum Framework {
    Test,
    Describe,
    Namespace,
    Expect,
    /// A function whose members in [`MEMBERS`] are all `undefined` (`test.each`).
    Leaf,
}

impl Framework {
    const MODIFIERS: [&str; 4] = ["skip", "only", "concurrent", "todo"];

    /// The member `member` of it: `None` when it is `undefined`.
    fn step(self, member: &str) -> Option<Framework> {
        let modifier = Self::MODIFIERS.contains(&member);
        match self {
            Framework::Test if modifier => Some(Framework::Test),
            Framework::Test if ["each", "for", "extend"].contains(&member) => Some(Framework::Leaf),
            Framework::Describe if modifier => Some(Framework::Describe),
            Framework::Describe if ["each", "for"].contains(&member) => Some(Framework::Leaf),
            Framework::Namespace if member == "test" => Some(Framework::Test),
            Framework::Namespace if member == "expect" => Some(Framework::Expect),
            Framework::Expect if member == "extend" => Some(Framework::Leaf),
            _ => None,
        }
    }

    /// Every member it has on some measured major, besides those of [`MEMBERS`]: own and
    /// inherited, walked up to `Function.prototype` on Vitest 3.2.7, 4.1.12 and 5.1.0.
    fn others(self) -> &'static [&'static str] {
        match self {
            Framework::Test => &[
                "afterAll",
                "afterEach",
                "aroundAll",
                "aroundEach",
                "beforeAll",
                "beforeEach",
                "describe",
                "fails",
                "fn",
                "mergeContext",
                "override",
                "runIf",
                "scoped",
                "sequential",
                "setContext",
                "skipIf",
                "suite",
                "withContext",
            ],
            Framework::Describe => &[
                "fn",
                "mergeContext",
                "runIf",
                "sequential",
                "setContext",
                "shuffle",
                "skipIf",
                "withContext",
            ],
            Framework::Namespace => &[
                "BenchmarkRunner",
                "EvaluatedModules",
                "Snapshots",
                "TestRunner",
                "afterAll",
                "afterEach",
                "aroundAll",
                "aroundEach",
                "assert",
                "assertType",
                "beforeAll",
                "beforeEach",
                "bench",
                "chai",
                "createExpect",
                "describe",
                "expectTypeOf",
                "inject",
                "it",
                "onTestFailed",
                "onTestFinished",
                "recordArtifact",
                "should",
                "suite",
                "vi",
                "vitest",
            ],
            Framework::Expect => &[
                "addEqualityTesters",
                "addSnapshotSerializer",
                "any",
                "anything",
                "arrayContaining",
                "assert",
                "assertions",
                "closeTo",
                "fail",
                "getState",
                "hasAssertions",
                "not",
                "objectContaining",
                "poll",
                "schemaMatching",
                "setState",
                "soft",
                "stringContaining",
                "stringMatching",
                "toBeFasterThan",
                "toBeOneOf",
                "toBeSlowerThan",
                "toSatisfy",
                "unreachable",
            ],
            Framework::Leaf => &[],
        }
    }

    /// The value of a framework atom: a function, the namespace, `undefined`, or nothing when
    /// reading through `undefined` or calling it throws. A member outside [`MEMBERS`] that some
    /// major has is a function or an object (measured `typeof` on each major), and what is read
    /// or called through it is not known, as is a call of a member that exists; a member no
    /// major has is `undefined`.
    fn value(atom: &Atom) -> Value {
        let unknown = Value {
            unknown: true,
            ..Value::default()
        };
        let mut at = Some(match atom.root {
            "test" => Framework::Test,
            "describe" => Framework::Describe,
            "v" => Framework::Namespace,
            _ => return unknown,
        });
        for step in &atom.steps {
            let Some(current) = at else {
                return Value::default();
            };
            let member = step.member.as_str();
            if !matches!(current, Framework::Namespace) && INHERITED.contains(&member) {
                return unknown;
            }
            if current.others().contains(&member) {
                return if step.arguments.is_none()
                    && atom
                        .steps
                        .last()
                        .is_some_and(|last| std::ptr::eq(last, step))
                {
                    Value {
                        function: true,
                        object: true,
                        ..Value::default()
                    }
                } else {
                    unknown
                };
            }
            at = if MEMBERS.contains(&member) {
                current.step(member)
            } else {
                None
            };
            if step.arguments.is_some() {
                // Measured on Vitest 3.2.7, 4.1.12 and 5.1.0: `test.extend()` has the members of
                // `test`, and `each()`/`for()` return a function with only `name` and `length`.
                match (at, current, member) {
                    (None, ..) => return Value::default(),
                    (Some(_), Framework::Test, "extend") => at = Some(Framework::Test),
                    (Some(_), Framework::Test | Framework::Describe, "each" | "for") => {
                        at = Some(Framework::Leaf);
                    }
                    (Some(_), ..) => return unknown,
                }
            }
        }
        match at {
            Some(Framework::Namespace) => Value {
                object: true,
                ..Value::default()
            },
            Some(_) => Value {
                function: true,
                ..Value::default()
            },
            None => Value {
                nullish: true,
                ..Value::default()
            },
        }
    }
}

/// How an expression wraps the value without changing it.
#[derive(Clone, Copy, Debug)]
enum Cast {
    Parenthesized,
    As,
    NonNull,
    Satisfies,
}

/// An expression over atoms, built only from JavaScript forms.
#[derive(Clone, Debug)]
enum Form {
    Atom(Atom),
    /// `'x'`
    Text,
    /// `1`
    Number,
    /// `on`, a boolean unknown statically.
    Flag,
    /// `(c ? a : b)`
    Conditional(Box<Form>, Box<Form>, Box<Form>),
    /// `(a && b)`
    And(Box<Form>, Box<Form>),
    /// `(a || b)`
    Or(Box<Form>, Box<Form>),
    /// `(a ?? b)`
    Nullish(Box<Form>, Box<Form>),
    /// `[a][0]`
    Index(Box<Form>),
    /// `({ t: a }).t`
    Member(Box<Form>),
    /// `(a, b)`
    Sequence(Box<Form>, Box<Form>),
    /// `(a).name` or `(a).length`: what JavaScript gives for each value `a` may be.
    Scalar(Box<Form>, &'static str),
    /// `identity(a)`: a call handed `a` that returns it.
    Call(Box<Form>),
    /// `[a, b][1]`: the second element.
    Pick(Box<Form>, Box<Form>),
    /// `({ t: a, u: b }).u`: the second property.
    Field(Box<Form>, Box<Form>),
    /// `[a, b].length`: the number 2.
    Count(Box<Form>, Box<Form>),
    /// `(a)`, `(a as any)`, `(a)!` or `(a satisfies unknown)`: `a` itself.
    Cast(Box<Form>, Cast),
}

/// Whether a direct call of each atom registers, keyed by the atom's source.
type Verdicts = HashMap<String, bool>;

impl Form {
    /// The expression as the module spells it.
    fn source(&self) -> String {
        match self {
            Form::Atom(atom) => atom.source(),
            Form::Text => "'x'".to_owned(),
            Form::Number => "1".to_owned(),
            Form::Flag => "on".to_owned(),
            Form::Conditional(test, then, otherwise) => format!(
                "({} ? {} : {})",
                test.source(),
                then.source(),
                otherwise.source()
            ),
            Form::And(left, right) => format!("({} && {})", left.source(), right.source()),
            Form::Or(left, right) => format!("({} || {})", left.source(), right.source()),
            Form::Nullish(left, right) => format!("({} ?? {})", left.source(), right.source()),
            Form::Index(value) => format!("[{}][0]", value.source()),
            Form::Member(value) => format!("({{ t: {} }}).t", value.source()),
            Form::Sequence(first, last) => format!("({}, {})", first.source(), last.source()),
            Form::Scalar(value, member) => format!("({}).{member}", value.source()),
            Form::Call(value) => format!("identity({})", value.source()),
            Form::Pick(first, second) => format!("[{}, {}][1]", first.source(), second.source()),
            Form::Field(first, second) => {
                format!("({{ t: {}, u: {} }}).u", first.source(), second.source())
            }
            Form::Count(first, second) => {
                format!("[{}, {}].length", first.source(), second.source())
            }
            Form::Cast(value, Cast::Parenthesized) => format!("({})", value.source()),
            Form::Cast(value, Cast::As) => format!("({} as any)", value.source()),
            Form::Cast(value, Cast::NonNull) => format!("({})!", value.source()),
            Form::Cast(value, Cast::Satisfies) => format!("({} satisfies unknown)", value.source()),
        }
    }

    /// Every atom in the expression, operands that are never the value included.
    fn atoms<'a>(&'a self, into: &mut Vec<&'a Atom>) {
        match self {
            Form::Atom(atom) => into.push(atom),
            Form::Text | Form::Number | Form::Flag => {}
            Form::Conditional(a, b, c) => {
                a.atoms(into);
                b.atoms(into);
                c.atoms(into);
            }
            Form::And(a, b)
            | Form::Or(a, b)
            | Form::Nullish(a, b)
            | Form::Sequence(a, b)
            | Form::Pick(a, b)
            | Form::Field(a, b)
            | Form::Count(a, b) => {
                a.atoms(into);
                b.atoms(into);
            }
            Form::Index(a)
            | Form::Member(a)
            | Form::Scalar(a, _)
            | Form::Call(a)
            | Form::Cast(a, _) => a.atoms(into),
        }
    }

    /// What the values it may evaluate to may be, by JavaScript alone. A conditional's test, the
    /// left of `&&` when truthy, the right of `||` behind a truthy left, the right of `??` behind
    /// a non-nullish left, the first of a sequence and the element or property not read are not
    /// the value.
    fn value(&self, verdicts: &Verdicts) -> Value {
        match self {
            Form::Atom(atom) if verdicts[&atom.source()] => Value {
                function: true,
                registers: Registers::Yes,
                ..Value::default()
            },
            Form::Atom(atom) => atom.plain_value(),
            Form::Text => Value {
                text: true,
                ..Value::default()
            },
            Form::Number => Value {
                truthy: true,
                ..Value::default()
            },
            Form::Count(first, second) => {
                if first.value(verdicts).throws() || second.value(verdicts).throws() {
                    Value::default()
                } else {
                    Value {
                        truthy: true,
                        ..Value::default()
                    }
                }
            }
            Form::Flag => Value {
                truthy: true,
                falsy: true,
                ..Value::default()
            },
            Form::Scalar(receiver, member) => receiver.value(verdicts).own(member),
            Form::Conditional(test, then, otherwise) => {
                let test = test.value(verdicts);
                let then = then.value(verdicts);
                let otherwise = otherwise.value(verdicts);
                if test.throws() {
                    test
                } else if test.always_truthy() {
                    then
                } else if test.always_falsy() {
                    otherwise
                } else {
                    then.join(otherwise)
                }
            }
            Form::And(left, right) => {
                let left = left.value(verdicts);
                let right = right.value(verdicts);
                if left.throws() {
                    left
                } else if left.always_truthy() {
                    right
                } else if left.always_falsy() {
                    left
                } else {
                    left.falsy_part().join(right)
                }
            }
            Form::Or(left, right) => {
                let left = left.value(verdicts);
                let right = right.value(verdicts);
                if left.always_truthy() {
                    left
                } else if left.always_falsy() {
                    right
                } else {
                    left.truthy_part().join(right)
                }
            }
            Form::Nullish(left, right) => {
                let left = left.value(verdicts);
                let right = right.value(verdicts);
                if !left.may_be_nullish() {
                    left
                } else if left.always_nullish() {
                    right
                } else {
                    left.non_nullish_part().join(right)
                }
            }
            Form::Index(value) | Form::Member(value) | Form::Call(value) | Form::Cast(value, _) => {
                value.value(verdicts)
            }
            // What precedes the value is evaluated first; when it throws, so does the whole.
            Form::Sequence(first, value) | Form::Pick(first, value) | Form::Field(first, value) => {
                if first.value(verdicts).throws() {
                    Value::default()
                } else {
                    value.value(verdicts)
                }
            }
        }
    }
}

/// Whether some value an expression may evaluate to registers when called.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Registers {
    #[default]
    No,
    /// A member of a value nothing here knows: JavaScript alone cannot say.
    Undecided,
    Yes,
}

/// The kinds of value an expression may evaluate to.
#[derive(Clone, Copy, Debug, Default)]
struct Value {
    /// A function: truthy, never nullish, with a string `name` and a number `length`.
    function: bool,
    /// An object that is not a function (a namespace): truthy, never nullish, with neither.
    object: bool,
    /// A non-empty string.
    text: bool,
    /// The empty string.
    empty: bool,
    /// A truthy primitive other than a string (`1`, `true`).
    truthy: bool,
    /// A falsy primitive other than a string (`0`, `false`).
    falsy: bool,
    /// `null` or `undefined`.
    nullish: bool,
    /// A value nothing here knows: any of the above.
    unknown: bool,
    /// A truthy value nothing here knows: what is read through it is not known either.
    opaque: bool,
    registers: Registers,
}

impl Value {
    /// Either value.
    fn join(self, other: Value) -> Value {
        Value {
            function: self.function || other.function,
            object: self.object || other.object,
            text: self.text || other.text,
            empty: self.empty || other.empty,
            truthy: self.truthy || other.truthy,
            falsy: self.falsy || other.falsy,
            nullish: self.nullish || other.nullish,
            unknown: self.unknown || other.unknown,
            opaque: self.opaque || other.opaque,
            registers: self.registers.max(other.registers),
        }
    }

    /// Whether every value it may be is truthy.
    fn always_truthy(self) -> bool {
        !self.empty && !self.falsy && !self.nullish && !self.unknown
    }

    /// Whether every value it may be is falsy.
    fn always_falsy(self) -> bool {
        !self.function
            && !self.object
            && !self.text
            && !self.truthy
            && !self.unknown
            && !self.opaque
    }

    /// Whether evaluating it always throws: it has no value at all.
    fn throws(self) -> bool {
        !self.function
            && !self.object
            && !self.text
            && !self.empty
            && !self.truthy
            && !self.falsy
            && !self.nullish
            && !self.unknown
            && !self.opaque
    }

    /// Whether some value it may be is `null` or `undefined`.
    fn may_be_nullish(self) -> bool {
        self.nullish || self.unknown
    }

    /// Whether every value it may be is `null` or `undefined`.
    fn always_nullish(self) -> bool {
        !self.function
            && !self.object
            && !self.text
            && !self.empty
            && !self.truthy
            && !self.falsy
            && !self.unknown
            && !self.opaque
    }

    /// The values that are falsy; a function is truthy, so none registers.
    fn falsy_part(self) -> Value {
        Value {
            empty: self.empty || self.unknown,
            falsy: self.falsy || self.unknown,
            nullish: self.nullish || self.unknown,
            ..Value::default()
        }
    }

    /// The values that are truthy; those of an unknown value are any truthy one.
    fn truthy_part(self) -> Value {
        Value {
            empty: false,
            falsy: false,
            nullish: false,
            unknown: false,
            opaque: self.opaque || self.unknown,
            ..self
        }
    }

    /// The values that are not nullish; those of an unknown value are any other one.
    fn non_nullish_part(self) -> Value {
        Value {
            empty: self.empty || self.unknown,
            falsy: self.falsy || self.unknown,
            nullish: false,
            unknown: false,
            opaque: self.opaque || self.unknown,
            ..self
        }
    }

    /// Reading `member` (`name` or `length`) of it: a function's are a string and a number, a
    /// string's `length` is a number as truthy as the string, and any other primitive has
    /// neither. Reading either of a
    /// nullish value throws, so it yields nothing.
    fn own(self, member: &str) -> Value {
        let any_string = Value {
            text: true,
            empty: true,
            ..Value::default()
        };
        let any_number = Value {
            truthy: true,
            falsy: true,
            ..Value::default()
        };
        let missing = Value {
            nullish: true,
            ..Value::default()
        };
        let mut read = Value::default();
        if self.function {
            read = read.join(if member == "name" {
                any_string
            } else {
                any_number
            });
        }
        if self.text || self.empty {
            read = read.join(if member == "length" {
                // A non-empty string's length is truthy, the empty string's is `0`.
                Value {
                    truthy: self.text,
                    falsy: self.empty,
                    ..Value::default()
                }
            } else {
                missing
            });
        }
        if self.truthy || self.falsy || self.object {
            read = read.join(missing);
        }
        if self.unknown || self.opaque {
            read = read.join(Value {
                unknown: true,
                registers: Registers::Undecided,
                ..Value::default()
            });
        }
        read
    }
}

/// Where the value is written relative to its call, and how it travels there.
#[derive(Clone, Copy, Debug)]
enum Location {
    /// `(value)('a', …)` in the test file.
    Inline,
    /// `const name = value` in the test file, then the call.
    Local,
    /// `export const name = value`
    Const,
    /// `export let name = value`
    Let,
    /// `export default value`, imported as `name`.
    Default,
    /// `const nameInner = value` exported `as name`.
    Renamed,
    /// `const nameInner = value` then `export const name = nameInner`.
    Alias,
    /// `export const { name } = { name: value }`
    ObjectPattern,
    /// `export const [name] = [value]`
    ArrayPattern,
    /// Exported, then re-exported by name from a second module.
    ReExport,
    /// Exported, then re-exported by `export *` from a second module.
    StarExport,
    /// Exported, then called as a member of a namespace import.
    Namespace,
}

const LOCATIONS: [Location; 12] = [
    Location::Inline,
    Location::Local,
    Location::Const,
    Location::Let,
    Location::Default,
    Location::Renamed,
    Location::Alias,
    Location::ObjectPattern,
    Location::ArrayPattern,
    Location::ReExport,
    Location::StarExport,
    Location::Namespace,
];

/// A project placing `form` at `location` under `name`: the test file, the other files, and the
/// test file's line holding the call, or, unless `called`, only the evaluation of the callee.
fn project(
    form: &Form,
    name: &str,
    location: Location,
    called: bool,
) -> (String, Vec<(&'static str, String)>, usize) {
    let value = form.source();
    let call = |callee: &str| {
        if called {
            // The prelude ends without a semicolon: a line starting with `(` would continue it.
            format!(";{callee}('a', () => {{}})\n")
        } else {
            format!(";({callee})\n")
        }
    };
    let module = |export: String| ("values.ts", format!("{PRELUDE}{export}\n"));
    let imported = |import: String, callee: &str| format!("{import}\n{}", call(callee));
    let prelude_lines = PRELUDE.lines().count();
    let (main, files) = match location {
        Location::Inline => {
            return (
                format!("{PRELUDE}{}", call(&format!("({value})"))),
                vec![],
                prelude_lines + 1,
            )
        }
        Location::Local => {
            return (
                format!("{PRELUDE}const {name} = {value}\n{}", call(name)),
                vec![],
                prelude_lines + 2,
            )
        }
        Location::Const => (
            imported(format!("import {{ {name} }} from './values'"), name),
            vec![module(format!("export const {name} = {value}"))],
        ),
        Location::Let => (
            imported(format!("import {{ {name} }} from './values'"), name),
            vec![module(format!("export let {name} = {value}"))],
        ),
        Location::Default => (
            imported(format!("import {name} from './values'"), name),
            vec![module(format!("export default {value}"))],
        ),
        Location::Renamed => (
            imported(format!("import {{ {name} }} from './values'"), name),
            vec![module(format!(
                "const {name}Inner = {value}\nexport {{ {name}Inner as {name} }}"
            ))],
        ),
        Location::Alias => (
            imported(format!("import {{ {name} }} from './values'"), name),
            vec![module(format!(
                "const {name}Inner = {value}\nexport const {name} = {name}Inner"
            ))],
        ),
        Location::ObjectPattern => (
            imported(format!("import {{ {name} }} from './values'"), name),
            vec![module(format!(
                "export const {{ {name} }} = {{ {name}: {value} }}"
            ))],
        ),
        Location::ArrayPattern => (
            imported(format!("import {{ {name} }} from './values'"), name),
            vec![module(format!("export const [{name}] = [{value}]"))],
        ),
        Location::ReExport => (
            imported(format!("import {{ {name} }} from './again'"), name),
            vec![
                module(format!("export const {name} = {value}")),
                ("again.ts", format!("export {{ {name} }} from './values'\n")),
            ],
        ),
        Location::StarExport => (
            imported(format!("import {{ {name} }} from './again'"), name),
            vec![
                module(format!("export const {name} = {value}")),
                ("again.ts", "export * from './values'\n".to_owned()),
            ],
        ),
        Location::Namespace => (
            imported(
                "import * as m from './values'".to_owned(),
                &format!("m.{name}"),
            ),
            vec![module(format!("export const {name} = {value}"))],
        ),
    };
    (main, files, 2)
}

/// How many sites the test file records on `line`.
fn sites_on(main: &str, files: &[(&'static str, String)], line: usize) -> usize {
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (*path, text.as_str()))
        .collect();
    let at = format!("@{line}:");
    sites(main, &files, false)
        .iter()
        .filter(|site| site.contains(&at))
        .count()
}

/// Whether calling the value at `location` records a site beyond those evaluating it records
/// (`test.skip()` registers where it is evaluated, whatever its result does when called).
fn call_registers(form: &Form, name: &str, location: Location) -> bool {
    let (main, files, line) = project(form, name, location, true);
    let (evaluated, evaluated_files, evaluated_line) = project(form, name, location, false);
    sites_on(&main, &files, line) > sites_on(&evaluated, &evaluated_files, evaluated_line)
}

/// The analyzer's verdict on a direct call of every atom in `form`: whether the call records a
/// site beyond those evaluating the atom records.
fn verdicts(form: &Form) -> Verdicts {
    let mut atoms = Vec::new();
    form.atoms(&mut atoms);
    atoms
        .into_iter()
        .map(|atom| {
            let registers = call_registers(&Form::Atom(atom.clone()), "a", Location::Inline);
            (atom.source(), registers)
        })
        .collect()
}

/// An identifier the prelude does not bind and no function owns.
fn name() -> impl Strategy<Value = String> {
    "[a-z][a-zA-Z0-9_]{0,7}"
        .prop_filter("a free identifier", |name| !TAKEN.contains(&name.as_str()))
}

/// Words a module cannot bind with `const` (reserved, strict-mode reserved, or TypeScript's).
const RESERVED: [&str; 48] = [
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// A [`name`] a module can bind.
fn binding() -> impl Strategy<Value = String> {
    name().prop_filter("a bindable identifier", |name| {
        !RESERVED.contains(&name.as_str())
    })
}

/// A member chain of up to two steps from a prelude root.
fn atom() -> impl Strategy<Value = Atom> {
    let member = prop_oneof![
        3 => proptest::sample::select(MEMBERS.to_vec()).prop_map(str::to_owned),
        1 => name(),
    ];
    let step = (
        member,
        proptest::option::of(proptest::sample::select(vec!["()", "({})", "([1, 2])"])),
    )
        .prop_map(|(member, arguments)| Step { member, arguments });
    (
        proptest::sample::select(ROOTS.to_vec()),
        proptest::collection::vec(step, 0..=2),
    )
        .prop_map(|(root, steps)| Atom { root, steps })
}

/// An atom, a literal, or the flag `on`.
fn leaf() -> impl Strategy<Value = Form> {
    prop_oneof![
        8 => atom().prop_map(Form::Atom),
        1 => Just(Form::Text),
        1 => Just(Form::Number),
        1 => Just(Form::Flag),
    ]
}

/// A leaf, or JavaScript forms nested up to three deep.
fn form() -> impl Strategy<Value = Form> {
    leaf().prop_recursive(3, 16, 3, |inner| {
        let boxed = inner.prop_map(Box::new);
        let pair = (boxed.clone(), boxed.clone());
        prop_oneof![
            3 => (boxed.clone(), boxed.clone(), boxed.clone())
                .prop_map(|(test, then, otherwise)| Form::Conditional(test, then, otherwise)),
            2 => pair.clone().prop_map(|(a, b)| Form::And(a, b)),
            2 => pair.clone().prop_map(|(a, b)| Form::Or(a, b)),
            1 => pair.clone().prop_map(|(a, b)| Form::Nullish(a, b)),
            1 => boxed.clone().prop_map(Form::Index),
            1 => boxed.clone().prop_map(Form::Member),
            1 => pair.clone().prop_map(|(a, b)| Form::Sequence(a, b)),
            1 => (boxed.clone(), proptest::sample::select(vec!["name", "length"]))
                .prop_map(|(value, member)| Form::Scalar(value, member)),
            1 => boxed.clone().prop_map(Form::Call),
            1 => pair.clone().prop_map(|(a, b)| Form::Pick(a, b)),
            1 => pair.clone().prop_map(|(a, b)| Form::Field(a, b)),
            1 => pair.prop_map(|(a, b)| Form::Count(a, b)),
            1 => (
                boxed,
                proptest::sample::select(vec![
                    Cast::Parenthesized,
                    Cast::As,
                    Cast::NonNull,
                    Cast::Satisfies,
                ])
            )
                .prop_map(|(value, cast)| Form::Cast(value, cast)),
        ]
    })
}

/// One of the locations.
fn location() -> impl Strategy<Value = Location> {
    proptest::sample::select(LOCATIONS.to_vec())
}

/// A call of a value is a registration site exactly when some value it may evaluate to is one,
/// wherever the value is written.
///
/// It describes the intended behaviour, so it fails while any generated shape is a gap.
#[test]
fn a_call_registers_exactly_when_some_value_it_may_evaluate_to_registers() {
    let decided = std::cell::Cell::new(0);
    property(
        512,
        (form(), binding(), location()),
        |(form, name, location)| {
            let verdicts = verdicts(&form);
            let expected = match form.value(&verdicts).registers {
                Registers::Yes => true,
                Registers::No => false,
                Registers::Undecided => return Ok(()),
            };
            decided.set(decided.get() + 1);
            let registered = call_registers(&form, &name, location);
            let (main, files, _) = project(&form, &name, location, true);
            prop_assert_eq!(
                registered,
                expected,
                "{} ({:?})\natoms: {:?}\n--- a.test.ts\n{}{}",
                if expected {
                    "a call that may register is not a site"
                } else {
                    "a call that cannot register is a site"
                },
                location,
                verdicts,
                main,
                files
                    .iter()
                    .map(|(path, text)| format!("--- {path}\n{text}"))
                    .collect::<String>()
            );
            Ok(())
        },
    );
    assert!(
        decided.get() >= 256,
        "{} of 512 cases decided: JavaScript alone decides too few",
        decided.get()
    );
}

/// Where a value is written and how it reaches its call does not change whether the call
/// registers. No model of JavaScript decides this one: every location must agree with the
/// value written inline.
#[test]
fn where_a_value_is_written_does_not_change_whether_its_call_registers() {
    property(256, (form(), binding()), |(form, name)| {
        let verdict = |location| call_registers(&form, &name, location);
        let inline = verdict(Location::Inline);
        for location in &LOCATIONS[1..] {
            prop_assert_eq!(
                verdict(*location),
                inline,
                "{:?} disagrees with the value written inline: {}",
                location,
                form.source()
            );
        }
        Ok(())
    });
}

/// Walk steps for exporting `value` and calling it once from the test file.
fn walk_steps(value: &str) -> usize {
    let module = format!("{PRELUDE}export const run: any = {value}\n");
    super::super::module::take_walk_steps();
    inventory_of(
        "import { run } from './values'\nrun('a', () => {})\n",
        &[("values.ts", module.as_str())],
        false,
    );
    super::super::module::take_walk_steps()
}

/// Deciding what a value may be reads each part of it a bounded number of times: holding the
/// value in both arms of a conditional at most doubles the work, plus slack for constant terms.
#[test]
fn holding_a_value_twice_at_most_doubles_the_work() {
    let measured = std::cell::Cell::new(0);
    property(256, form(), |form| {
        let once = form.source();
        let single = walk_steps(&once);
        measured.set(measured.get() + single);
        let double = walk_steps(&format!("(on ? {once} : {once})"));
        prop_assert!(
            double <= 2 * single + 64,
            "{single} steps for {once}, {double} for it twice"
        );
        Ok(())
    });
    assert!(measured.get() > 0, "no case walked a step");
}
