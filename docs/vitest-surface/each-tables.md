# Parameterized tables: .each and .for

Title derivation for these tables is on [Title formatting](title-formatting.md).

Three table shapes:

1. **Array of arrays**, each inner array is spread positionally into the callback params:
   ```ts
   test.each([[1, 1, 2], [1, 2, 3], [2, 1, 3]])('add(%i, %i) -> %i', (a, b, expected) => {
     expect(a + b).toBe(expected)
   })
   ```
   Row `[1, 1, 2]` + template `'add(%i, %i) -> %i'` → title `add(1, 1) -> 2`.

2. **Array of objects**, each object is passed whole; `$field` / `$0`,`$1`.. / `$field.nested`
   interpolation reads properties off it for the title, and the callback destructures fields by name:
   ```ts
   test.each([{ a: 1, b: 1, expected: 2 }])('add($a, $b) -> $expected', ({ a, b, expected }) => {
     expect(a + b).toBe(expected)
   })
   ```
   Also supports positional `$0`, `$1`, ... and dotted paths (`$a.val`).

3. **Tagged-template-literal table**, a string header row (`|`-separated column names) followed
   by one-or-more rows of `${value}` cells, consumed as a tagged template:
   ```ts
   test.each`
     a      | b      | expected
     ${1}   | ${1}   | ${2}
     ${'a'} | ${'b'} | ${'ab'}
   `('returns $expected when $a is added $b', ({ a, b, expected }) => {})
   ```
   Statically: the header row's identifiers become field names; the `${...}` expressions are the
   `TaggedTemplateExpression`'s `expressions` array, positionally matched to header columns in
   row-major order. Expressions with a statically proven value, including earlier local `const`
   aliases, can yield a concrete title. Unresolved expressions leave the row's value and title
   unknown, so the row remains opaque/dynamic.

**`test.for` vs `test.each`**: identical table syntax and identical title-derivation rules; the only
difference is at the callback-argument level, `.for` does **not** spread an array row into
positional params, it passes the whole row as the callback's first argument (so array rows require
destructuring: `([a, b, expected]) => {}`), and the callback receives `TestContext` as its **second**
parameter, enabling `context.expect` for concurrent-safe snapshots. Non-array (object / tagged
template) rows behave identically between `.each` and `.for`.

`describe.each` / `describe.for` use the same table/title rules to generate multiple `describe`
blocks (each with its own interpolated title and independently nested tests) rather than multiple
tests.

**Global primitive cells**: unshadowed `undefined`, `NaN`, and `Infinity` are readable values,
including signed infinity and earlier local `const` aliases. A local or imported binding with one
of these names must prove its own value; the spelling alone never identifies a global. Distinct
value kinds remain distinct row identities. Titles follow their formatter: array string coercion
renders `undefined` as empty, JSON array elements become `null`, and JSON object properties with
that value are omitted.

**Imported constants**: a cell naming a value import (`import { MODE } from './modes'`), or a
local `const` alias of one, is read when the import resolves, through named re-exports, to a
first-party module that declares it with a module-level `const` whose initializer is a string,
number, signed number, boolean, `null`, bigint or substitution-free template literal, directly or
through an earlier local `const` alias (never through a member read). The value then reads exactly
like the literal. Everything else stays unresolved: `let`/`var` and `declare` exports, computed
initializers (`'a' + 'b'`), `undefined`/`NaN`/`Infinity`, objects, arrays, calls, enum and object
members, default and namespace imports, `export *`, declaration files, installed packages, and an
import whose chain returns to the test file. An import is not read when the test file mocks a
module or re-export hop on its chain (`vi.mock`, or a `mockNuxtImport` of a name on the chain). A
file reads no imported constant at all when its mocking cannot be fully read (a `vi.doMock`, a
nested or unreadable `vi.mock`, a mocked specifier that does not resolve), or when it can redefine
a property accessor, which is how a module object's export is rewritten however the object was
reached: a `spyOn` given an access type (`vi.spyOn(ns, 'X', 'get')`) or used other than as a
direct call, or any `defineProperty`, `defineProperties`, `__defineGetter__` or
`__defineSetter__`. Mocks and accessor redefinitions outside the test file, in `setupFiles` or in a
module the test file imports, are not seen: the analyzer then reads the real value where Vitest
uses the replacement.

**Static-analysis fallback rule**: whenever the table itself is not a literal array/object
expression at the call site (e.g. `test.each(getCases())`), the extractor cannot enumerate rows at
all, it must record "parameterized, table unresolved" rather than guessing a row count.

Back to the [Vitest surface index](../vitest-surface.md).
