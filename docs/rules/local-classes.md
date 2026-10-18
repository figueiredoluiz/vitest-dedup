# Local classes and constructors

How tests that construct classes declared in the same file are compared.

## What counts as the same

### Constructor calls and classes

- A constructor call keeps its callee, its ordered arguments and its generic arguments.
- A local class declaration, class expression or owned alias keeps its implementation and its
  nominal identity. Two distinct class objects do not become interchangeable merely because their
  methods look the same.
- Fields, methods, accessors, private names, heritage and static initialization all take part in
  the comparison. Definition-time effects stay separate from construction.
- References to separate local instances keep their declaration identity even when their classes and
  construction arguments are identical.
- Consistent local renaming stays comparable.
- Parentheses and erased TypeScript `as`, `satisfies`, non-null and angle-bracket assertions on
  these paths keep the same constructor behavior.

### Captured values

A test body can use values declared outside it, and readable `.each` cells.

- A capture keeps which enclosing variable it uses, even when two variables start with equal values.
  Consistently renaming those variables stays comparable.
- Proven direct assignments update a captured scalar or constructor alias.
- Captured arrays and objects keep their initializer and their supported straight-line receiver
  mutations. Mutable captured objects keep one ordered history across proven owned constructions and
  mutations.
- Literal-only variations can still produce a `parameterization-candidate`; that finding says to
  preserve both cases.
- Declared functions and expression or arrow closures share owned lexical capture environments.
  Ordinary helper renaming stays comparable until its name is observed, and callable aliases keep
  the original function name.
- Vitest mock captures and plain TypeScript parameter properties are modeled.

### Names that stay values

- Known class names, intrinsic constructor names and observed stable callable names.
- Observed private method and callable-field names. Explicit function names survive private storage
  renaming.
- Ordinary private object data properties named `name` or `toString` stay comparable.

## What is left out of comparison

State the analyzer cannot model makes the dependent body *unresolved*, and an unresolved body is
excluded from comparison:

- dynamic control-flow writes
- escaping captured objects
- unknown class-object mutations
- dynamic reflective overrides
- source-text reflection
- decorators
- unowned factory results
- replaced or escaping private callables
- callable metadata writes or escapes
- a helper that returns a captured mutable object

Only the consumers are affected: unrelated tests remain comparable, and unresolved hooks and
fixtures affect only the tests that use them. Bounded class-summary and history caches degrade
visibly when their budgets are exceeded.

Native and imported constructor bindings keep their existing syntactic comparison. Their
conditional writes do not enter the owned local-class state model.

## Loop initializers and templates

Behavior collection visits `for` expression and declaration initializers and ordinary template
interpolations. It keeps nested calls, constructors and other modeled effects in source order and in
deferred scope. Tagged-template call modeling is separate.

Back to [Rules](../rules.md).
