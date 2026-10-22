# Imported runners

A runner is a first-party function that registers tests when called. VitestDedup expands a call to
one as if its tests were written at the call site, so wrapper helpers do not hide duplicates.

## What is extracted

The frontend extracts static declarations from called first-party function exports, including named
re-export chains.

- Source locations and fingerprints come from the helper.
- Registration ownership, governing globals and configuration, and surrounding hooks come from the
  caller.
- Repeated calls count as registrations but retain their shared declaration identity.
- Type-only imports, interfaces and type aliases are erased. They neither add module effects nor
  supply runtime runner or Vitest bindings, and type-only re-exports cannot expose a runtime runner.

## Accepted shapes

A runner is one of:

- a synchronous function declaration
- a `const` arrow or function-expression export (the first declarator of its statement), with a block or concise body
- a default export of a function or arrow
- `export default name`, where `name` is one of the above declared in the same file (a `const`
  must precede the export statement; a function declaration may follow it)
- an `export { local as name }` alias of those, whether or not the target is itself exported

It has plain positional parameters, static `test` and `describe` titles, and inline callbacks.
Suite callbacks must be synchronous with no parameters. Extra inert arguments at the call site are
ignored.

### Titles

A title may read a parameter when the call site's argument is one of:

- a literal or constant expression
- a loop variable of an unrolled `for...of`
- a value forwarded from a local helper

Each call renders its own title.

### Hooks

Hooks the runner registers (`beforeAll`, `beforeEach`, `afterEach`, `afterAll`, with an inline
callback and at most a literal timeout) attach to the calling scope as if written at the call site.
They keep the runner's source location and fingerprint, and apply to tests declared before or after
the call. A hook inside the runner's own `describe` stays in that suite.

### Fixtures

A `const t = test.extend({...})` at the top level of the runner, with static fixture values,
supplies fixtures to `t(...)` declarations. It accepts scoped `t.beforeEach(...)`-style hooks like
the bare ones. One declared inside a nested suite callback is refused.

### Nested calls

A runner may call another first-party runner or a function declared in its own file. The nested call
is expanded the same way, under the same depth, admission and source budgets, and a refusal inside
it refuses the outer runner. Recursion through modules or local helpers is refused.

## Refused

- A missing, spread or unprovable argument that a title reads.
- Default, rest or destructured parameters, and a parameter the runner writes.
- Registration modifiers, control flow and registration options.
- `let` or `var` exports, reassigned exports, async and generator functions, opaque initializers,
  and bodyless or overload signatures.
- A default export of a name that is not a readable runner in the same file: an imported binding
  (a further hop), a class or other value, a `const` declared after the export statement, or a function declaration
  the module reassigns.
- Module setup effects.
- Called opaque imports at registration time. Uncalled imports and calls inside test callbacks are
  not registration candidates.
- Named or default object method calls as runner candidates, and function `.call()` / `.apply()`
  invocations.
- Mocked runners and mocked re-export hops: they cannot contribute their real bodies.
- Installed-package bodies that are runners. A package whose manifests name no test runner can be
  part of a setup helper (see below); a package that registers tests is refused.

Proved ordinary utilities and shadowed inert callbacks contribute no declarations.

### Setup helpers

A call to an imported function that registers no test or hook is setup, not a runner. The call is
proved harmless when nothing it can run reaches Vitest's registration API (`test`, `it`, `describe`,
`suite`, `bench`, `beforeAll`, `beforeEach`, `afterAll`, `afterEach`, `onTestFinished`,
`onTestFailed`, `aroundEach`, `aroundAll`) and its arguments carry none either. The proof follows
what the code reads:

- Importing a module runs its top-level initializers (enum members included), never the bodies of
  functions it merely declares; calling an export runs that export. Everything the walk reads joins
  it: local declarations, imports by module and export, globals.
- A top level that is not declarations (a bare call, `if`, a loop) is refused. `vi.mock(...)`,
  `vi.fn()` and other `vi` or `expect` calls are setup, not registration.
- A name taken from `vitest` is judged by what it is, however it is re-exported: `test`, `it`,
  `describe`, `suite`, `bench`, `beforeAll`, `beforeEach`, `afterAll`, `afterEach`,
  `onTestFinished`, `onTestFailed`, `aroundEach` and `aroundAll` register; the namespace import does.
- A name the host does not define is unknown unless the project's `.nuxt/imports.d.ts` lists it;
  see [Nuxt projects](#nuxt-projects). `#imports` is that list under another spelling. `globalThis`,
  `eval`, `require`, a computed `import()`, `import.meta.glob` and `import.meta.vitest` reach code by name and are
  refused, except one named property of the global object (`globalThis.ready = true`). Node's `vm`,
  `child_process`, `worker_threads`, `module`, `repl`, `cluster` and `inspector` run code from text
  or elsewhere and are refused.
- The arguments of the call are held to the same rule, and a binding they read must not be written
  after it is declared: a callback reassigned, or a config changed through a member (`config.mode =
  2`, `items[0]++`, `delete config.mode`), is refused. Calls that change a value through a method
  (`items.push(1)`) are not tracked.
- Installed packages are judged by their manifests: one whose whole dependency closure names no
  test runner (`vitest`, `vitest-*`, `@vitest/*`, `jest`, `@jest/*`, `mocha`, `jasmine`,
  `playwright`, `playwright-core`, `@playwright/*`), and whose imported file and the scripts it loads
  by relative path (`require('./x')`, `from './x'`, `import('./x')`, up to 1,024 files and 32 MiB)
  do not mention `vitest`, cannot register a test. When the project enables Vitest globals those
  files must also not call a registration global as a free name. The loads are read from each
  script's syntax tree. A name that could be several files (`./x` as `x.js`, `x.mjs` or `x.ts`, or
  a directory) is checked against every one of them, and directories are followed through their
  `package.json` `main` and index. A package is refused when it is past those bounds, when a script
  cannot be parsed, when a load cannot be read (a computed or template specifier, `require` used as
  a value), or when a load leaves the package, needs a manifest that cannot be read, or is spelled in a
  way that cannot be followed (`#internal` and absolute specifiers, `?` or `#` suffixes, escapes). An
  unreadable manifest or a missing dependency is
  refused. `mountSuspended`, `renderSuspended`, `registerEndpoint`, `mockNuxtImport` and
  `mockComponent` from `@nuxt/test-utils/runtime` are known by name.
- An `import` whose bindings are never read as values and that names no module on disk (a package
  that ships only types) is ignored, as TypeScript erases it; a project that keeps such imports
  (`verbatimModuleSyntax`) is not covered by that assumption.
- Single-file components contribute their script blocks; `<script setup>` runs when mounted, not
  when imported. Styles, images, fonts and JSON carry no code.

The proof is lexical and aimed at code that is not written to evade it. It does not follow a value
through an alias (`const alias = config; alias.mode = 2`), a method that mutates (`items.push(1)`), a
registration global reached by another name (`const register = test`), reflection
(`process.getBuiltinModule('vm')`, `({}).constructor.constructor(...)`) or a mock of a package that
the helper imports several modules away. Those cases can make a helper that registers read as setup.

A setup call changes what every test in the file runs under, so it joins each test's context, in
call order and once per call: the helper's module and export, how many framework setup statements
(`vi.stubGlobal(...)`, awaited calls) precede it, the call's text, the text of every local
declaration the arguments read, the module and export each imported name stands for, and the file
`import.meta` stands for. Files that call different helpers, with different arguments, in a
different order or a different number of times do not compare equal; files that set up the same way
do. A mocked helper, an argument the proof cannot place,
and a helper that registers anything are refused as before.

### Module dependencies

Importing a module runs it, called or not. Module dependencies must be Vitest, the validated named
re-export chain, or first-party modules that are inert or that provably register nothing (see
[Setup helpers](#setup-helpers)); effects other than registration at import time are accepted and
are attributed through the runner's identity. Unknown dependency or setup effects, and helper
mocking effects, mark the entire calling file unresolved.

### What a refusal does

A refusal marks the file incomplete and the affected calling scope unresolved, including earlier
tests and its suite record. Those records are excluded from comparison. The `json` and `jsonl`
reports list the file with a reason code naming the refusal; see
[Excluded files](incomplete-runs.md#excluded-files).

## Nuxt projects

A Nuxt project is supported where the generated files are present: run `nuxt prepare` (the
[missing-prerequisite notice](incomplete-runs.md#missing-prerequisites) names it when they are not).

- Unimported names in helpers resolve from the nearest `.nuxt/imports.d.ts`: a first-party export
  joins the proof, a library entry must pass the package check. Without the file, or for a name it
  does not list, the name is unknown and the helper is refused.
- `mockNuxtImport(name, factory)` from `@nuxt/test-utils/runtime` at the top level of a test file is
  a hoisted mock of that auto-imported binding, keyed `nuxt-import:<name>`; its factory must be
  inline and must not itself reach a registration API. Anywhere else, or with an unreadable
  factory, the file is unresolved, as for `vi.mock`.
- Aliases such as `~/` resolve through `.nuxt/tsconfig.json`.

Component auto-imports, `nuxt.config` settings and auto-imports a module registers outside the
generated list are not read.

## Budgets

- Local and imported expansion share a depth of 8 and 2,000 body admissions across both passes.
- One setup-helper proof visits at most 64 modules, and reads at most 256 manifests for each
  installed package it reaches.
- The text check of an installed package follows relative `require`, `from` and `import()` to at
  most 1,024 scripts and 32 MiB (8 MiB each); a package past that is refused.
- Helper files share the source byte limit (8 MiB), the bracket nesting limit (256) and the
  export-hop limit (8).
- Exact helper sources retained for one caller share an additional 8 MiB budget; repeated calls
  retain each source once.
- Refusals produce at most one helper diagnostic per calling file.

Helper source bytes also serve inline suppressions and HTML snippets.

## Baselines

`--baseline-from-ref` currently refuses imported runners: historical helper blobs are unavailable,
so using present-day bodies would produce a false baseline. The base analysis is reported as
partial. File baselines from complete current analyses retain imported source and caller identity
normally.

Back to the [documentation index](README.md).
