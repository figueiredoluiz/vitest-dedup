# Imported runners

A runner is a first-party function that registers tests when called. When VitestDedup can prove
what a call registers, it expands the call into those tests, as if they were written at the call
site, so a wrapper helper does not hide duplicates. When it cannot, the call is counted and the
tests around it are left out of comparison rather than guessed at.

## What an expansion keeps

- Source locations come from the runner; the registering file, governing globals and configuration
  come from the caller.
- Each call registers its own tests, titled from its own arguments. Repeated calls of one runner
  keep the runner's declaration identity, so they are not reported as duplicates of each other.
- An expanded test is compared with every other test, including the caller's own.
- Reads of `import.meta` retain the runner's source module identity. A matching spelling in a
  different module does not establish equal metadata, even when both assertions happen to pass.
- Type-only imports, interfaces and type aliases are erased and add no effects.

## When a call is expanded

The calling file qualifies when it parses cleanly and its top level holds only imports, inert
declarations (functions, types, literal constants), plain `test(title, callback)` calls with a
static title (a string of any length, or a template up to 65,536 bytes) and an inline callback,
statement-level calls of imported runners or [same-file helpers](#same-file-helpers), hook
registrations, and plain `describe` blocks (a literal title, no `each`, no `skip`) whose bodies
close the same way. A call may sit inside such a `describe`: Vitest runs describe callbacks after
the module body, so what the call registers joins that suite, and the tests take its suite path.
A table (`each`) or skipped (`skip`) suite around a call, a hook or label the sequence cannot
close around, a module load the file does not import by name, or a registration function read as
a value refuses the whole file's expansion. Every value it imports from a project module must be
a runner the proof read.

A runner qualifies when:

- it is a named or default export, reached directly or through re-export hops, whose binding is
  never reassigned;
- it is a synchronous, non-generator function with a body that does not read `this` or `arguments`,
  with plain, immutable positional parameters and no direct `eval`;
- importing its module, and every project module that loads with it, runs nothing that is not
  inert;
- its body is a sequence of `test(title, callback)` calls with an inline callback, hook
  registrations, and calls of other runners, local or imported, that qualify in turn. A `describe`
  inside a runner is not expanded yet. Statements after an unconditional `return` do not run.
  The sequence may also hold:
  - a `const` holding one of those registrations (`const registered = test(...)`);
  - a `try` without `catch` whose `finally` declares nothing, reads only literals and constants and
    ends normally, which leaves the block's registrations as they are.

A function declared with overload signatures runs its implementation. A registration function
built inside a runner (`const each = test.each([1])`) refuses, even when it is never used:
building one changes state Vitest's later registrations read. So does the in-source binding
`import.meta.vitest`, which an imported module may not receive.

A title may read the call's arguments when they are literals or constants the proof can evaluate.

## Same-file helpers

A function in the calling file is expanded the same way when:

- it is a top-level `function` declaration, or a top-level `const` bound to a function or arrow,
  never reassigned;
- every use of it is a call: a statement-level call at the top level, or a call from another helper
  that qualifies;
- a `const` helper is declared before the first top-level call, since a hoisted function called
  above the declaration would reach it uninitialized. A call inside a `describe` runs after the
  whole module body, so it is exempt; the top-level call is what orders the check;
- its body meets the runner rules above.

A test written inside such a helper is registered once per call, titled from that call's arguments,
and is not counted again on its own. A helper that fails any rule leaves the file's tests excluded,
as before.

## Hooks-only helpers

A call of a helper whose body registers only hooks is a `hook-wrapper` site: it registers no test,
and its hooks join the context of the tests in its scope. A body counts as hooks-only only when it is
closed: plain parameters, and statements that are hook registrations, calls of other hooks-only
helpers, declarations of plain names, `return`, blocks and `if` over values that run nothing. Proven
lexical `beforeEach` setup an imported helper installs takes part in `redundant-hook`; see
[Hook scheduling](rules.md#hook-scheduling).

## Setup calls and packages

A call that registers nothing, of a first-party function or of an installed package, is setup: it
joins the context of every test in its scope, so files that call it pair only with files that make
the same call. Installed-package code is not read. A package that registers tests when called is not
seen: its tests are missing from the count.

## What a refusal does

Each call that may register tests and is not expanded counts as one test excluded as
`registered-elsewhere`, and the scope around it (its suite, or the file) is excluded as
`helper-effects-unproved`. The run is marked incomplete, and the `json` and `jsonl` reports list the
file with its reason; see [Excluded files](incomplete-runs.md#excluded-files).

## Nuxt projects

Aliases such as `~/` resolve through `.nuxt/tsconfig.json`; run `nuxt prepare` when the
[missing-prerequisite notice](incomplete-runs.md#missing-prerequisites) names it. The generated
auto-import tables (`.nuxt/imports.d.ts`) are not read: an auto-imported call is setup like any
other call that registers nothing.

## Budgets

- Expansion follows runner calls to a depth of 8 and at most 2,000 calls per file, each with at most
  2,000 arguments.
- The helper sources one file's expansion reads share 8 MiB, and its proof work is bounded at
  8 MiB.
- Helper files share the source limits: 8 MiB, 36,864 raw syntax units and lexical nesting depth
  256.
- Each module inventory also bounds uncached import analysis at depth 8, 64 module-analysis
  attempts and 8 MiB of source. A limit makes the inventory incomplete and prevents runner
  expansion from using its partial results; completed cached proofs remain reusable.

A file past a budget is refused as a whole, with the outcome above.

## Baselines

`--baseline-from-ref` resolves project imports against committed source and resolver
configuration, with the same import-classification boundaries as a working-tree run. It never substitutes current helpers for historical dependencies.
Historical symlinks and actual source/configuration access failures refuse the baseline. A
missing package-based tsconfig extension or unresolved linkage to a committed project package makes
the historical pass incomplete with no allowance. Private `#`-mapped tsconfig extensions remain
errors; see
[revision baselines](baselines.md#baseline-from-a-revision).

Back to the [documentation index](README.md).
