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
   row-major order. Only const-foldable expressions in the cells yield a static concrete title;
   non-literal expressions (e.g. `${someVar}`) mean the row's concrete value, and therefore its
   title, is not statically known and must be treated as an opaque/dynamic row.

**`test.for` vs `test.each`**: identical table syntax and identical title-derivation rules; the only
difference is at the callback-argument level, `.for` does **not** spread an array row into
positional params, it passes the whole row as the callback's first argument (so array rows require
destructuring: `([a, b, expected]) => {}`), and the callback receives `TestContext` as its **second**
parameter, enabling `context.expect` for concurrent-safe snapshots. Non-array (object / tagged
template) rows behave identically between `.each` and `.for`.

`describe.each` / `describe.for` use the same table/title rules to generate multiple `describe`
blocks (each with its own interpolated title and independently nested tests) rather than multiple
tests.

**Static-analysis fallback rule**: whenever the table itself is not a literal array/object
expression at the call site (e.g. `test.each(getCases())`), the extractor cannot enumerate rows at
all, it must record "parameterized, table unresolved" rather than guessing a row count.

Back to the [Vitest surface index](../vitest-surface.md).
