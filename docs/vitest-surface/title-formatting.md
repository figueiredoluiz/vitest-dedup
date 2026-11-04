# Title formatting

How Vitest renders a `.each` / `.for` title, and how far this analyzer reproduces it. The table
shapes are on [`.each` and `.for` tables](each-tables.md).

**Title interpolation directives** (printf-style, applied left-to-right against the row's
positional values, independent of `$field` mode):

| Directive | Meaning |
| --- | --- |
| `%s` | string |
| `%d` | number |
| `%i` | integer |
| `%f` | float |
| `%j` | JSON |
| `%o` | object |
| `%#` | 0-based index of the row within the table |
| `%$` | 1-based index of the row |
| `%%` | documented as a literal `%`; **measured otherwise, see below** |

## What the numeric directives actually coerce

Measured against **Vitest 5.0.3**, one value per row, reading each test's own `currentTestName` from
inside the run rather than inferring it from the pattern. `%d` is `Number(value)`; `%i` and `%f` are
`parseInt`/`parseFloat` over the value's **string** form, which is a different function on a different
input and disagrees with `%d` on most non-numbers.

| value | `%s` | `%d` | `%i` | `%f` |
| --- | --- | --- | --- | --- |
| `null` | `null` | `0` | `NaN` | `NaN` |
| `true` | `true` | `1` | `NaN` | `NaN` |
| `false` | `false` | `0` | `NaN` | `NaN` |
| `undefined` | `undefined` | `NaN` | `NaN` | `NaN` |
| `0` | `0` | `0` | `0` | `0` |
| `-0` | `-0` | `0` | `0` | `-0` |
| `1.5` | `1.5` | `1.5` | `1` | `1.5` |
| `-1.5` | `-1.5` | `-1.5` | `-1` | `-1.5` |
| `'a'` | `a` | `NaN` | `NaN` | `NaN` |
| `''` | *(empty)* | `0` | `NaN` | `NaN` |
| `'1.5'` | `1.5` | `1.5` | `1` | `1.5` |
| `'12abc'` | `12abc` | `NaN` | `12` | `12` |
| `'0x1F'` | `0x1F` | `31` | `31` | `0` |
| `[]` | *(empty)* | `0` | `NaN` | `NaN` |
| `[2]` | `2` | `2` | `2` | `2` |
| `[2, 3]` | `2,3` | `NaN` | `2` | `2` |
| `[null]` | *(empty)* | `0` | `NaN` | `NaN` |
| `[true]` | `true` | `NaN` | `NaN` | `NaN` |
| `{ k: 1 }` | `{ k: 1 }` | `NaN` | `NaN` | `NaN` |
| `NaN` | `NaN` | `NaN` | `NaN` | `NaN` |
| `Infinity` | `Infinity` | `Infinity` | `NaN` | `Infinity` |
| `10n` | `10n` | `10n` | `10n` | `10` |

Four rows are worth naming because they are the ones a reimplementation gets wrong:

- **`null` and `true` are `0` and `1` under `%d` only.** Under `%i`/`%f` they are `NaN`, because
  `parseInt('null')` is `NaN`. A summary that says "`null` renders 0" is true of one directive in
  three.
- **`%i`/`%f` parse a prefix; `%d` does not.** `'12abc'` is `12` under `%i`/`%f` and `NaN` under `%d`.
- **An array goes through its string form.** `[2, 3]` is `'2,3'`, so `%i` and `%f` yield `2` while
  `%d` yields `NaN`. `[]` is `''`, which `Number` reads as `0` but `parseInt` reads as `NaN`.
- **`-0` keeps its sign under `%f` alone**, Vitest's own `formatTitle` rewrites the directive to
  `-%f` for a negative-zero item (read from the 5.0.3 bundle), while `%d`/`%i` print `0`. A BigInt
  is *never* `Number(value)`: `%d`/`%i` special-case it to print the literal's own `10n`, while
  `%f` has no such case and prints `parseFloat('10')` = `10`.

**What this analyzer renders, and where it refuses.** The title renderer (`src/normal/titles.rs`)
proves `%s` and `$field` over string and number literals. The numeric directives over non-finite
values, global `undefined`, `NaN` and `Infinity`, `%o`, `%j` and composites are not proven: the
title stays unrendered and the declaration opts out of title comparison, because a rendering the
analyzer cannot prove is not a title it may compare.

`%s` is `String(value)` except for plain objects, which get `pretty-format` (`{ k: 1 }`, not
`[object Object]`). `%o` is the inspect form throughout (`'a'`, `[ 2, 3 ]`).

## `%%` consumes an argument (upstream defect)

Documented as a literal `%`. Measured on Vitest 5.0.3, it emits `%`, a space, and **the next
positional argument**, consuming it, so every directive after it shifts by one:

| pattern | row | rendered title |
| --- | --- | --- |
| `%% x %s y %s z %s` | `['A','B','C']` | `% A x B y C z undefined` |
| `x %s y %% z %s` | `['A','B','C']` | `x A y % B z C` |
| `x %s y %s z %%` | `['A','B','C']` | `x A y B z % C` |
| `%%%%` | `['A','B','C']` | `% A% B` |
| `100%% done %s` | `['A','B']` | `100% A done B` |
| `only %s` | `['A','B','C']` | `only A` |

The last row rules out "leftover arguments are appended": they are not. `%%` is behaving as a
directive, not as an escape.

**This analyzer refuses to render a title containing `%%`** rather than reproducing this, because
it cannot know which Vitest version will run the tests it is reading. It parses configuration and
source without executing either, so the installed version is not a fact available to it. Mirroring
the current behaviour would produce wrong titles for anyone on a version where this is fixed, and a
wrong title is a wrong answer about which rows collide. Refusing costs only that rows with `%%` in
the title stay uncompared, and the run says so.

## Depth truncation and width elision are one 40-character budget

Measured on **Vitest 5.0.3** by the same probe method (one `.each` row per value, reading each
test's own `currentTestName`), cross-checked by calling the shipped `inspect` directly, the two
agree on every value below. The probe is permanent: `scripts/check/verify-titles.mjs` runs
`scripts/vitest-harness/title-values.test.ts` through the pinned Vitest and through this analyzer
and fails on any title that differs. Every row of the table below is in that fixture except the
two nested past `MAX_CELL_DEPTH`, which this analyzer refuses: a refusal has no runtime title to
compare. There is **no fixed depth limit and no fixed item-count limit**: both
truncations are driven by `taskTitleValueFormatTruncate` (default **40 characters**), applied per
interpolated value. The algorithm, read from the 5.0.3 bundle's `inspect`/`stringify` and confirmed
by the measurements:

1. Render the value with pretty-format at `maxDepth: 10`.
2. While the result is **≥ 40 units** and `maxDepth > 1`, halve `maxDepth` (10 → 5 → 2 → 1)
   and re-render. A composite past `maxDepth` prints as `[Array]`/`[Object]`.
3. If it is still > 40: a *string* keeps its first 38 units inside the quotes, then `…`; an
   **array, object, set or map** re-renders at `maxDepth: 1` with a binary-searched item width so
   the result fits 40, marking the cut `…(k)` where `k` is the number of omitted entries; anything
   else re-renders at `maxDepth: 0`.

Every length is JavaScript `string.length`: UTF-16 code units, so `😀` counts twice, and a cut
that would split a surrogate pair backs off one unit. A bare `$field` string takes a different
path, `truncateString(value, 40)`: unchanged at 40 units, otherwise the first 39 then `…`. `%s`
of a plain object is the same `inspect` call as `%o` (the object has no own `toString`); `%s` of
a string or an array is not truncated at all.

| value interpolated into the title | rendered (Vitest 5.0.3, truncate 40) |
| --- | --- |
| `[0..9]`, 10 one-digit items (32 chars) | `[ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9 ]`, full |
| `[0..19]`, 20 items | `[ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, …(10) ]` |
| `[90..100]`, 11 two-digit items | `[ 90, 91, 92, 93, 94, 95, 96, 97, …(3) ]`, the width is *searched* to fit the budget, not a fixed 10 |
| `{ <46-char key>: 1 }` | `{ …(1) }`, the search can find width 0 |
| array nested 9 deep (37 chars) | full, depth alone never truncates |
| array nested 10 deep (41 chars) | `[ [ [ [ [ [Array] ] ] ] ] ]`, halving landed at 5 levels |
| object nested 5 deep (37 chars) | `{ a: { a: { a: { a: { a: 1 } } } } }`, full |
| object nested 6 deep (42 chars) | `{ a: { a: [Object] } }` |
| the string `'a'×38` under `%o` | `'aaaa…aaa'`, full: 40 units with the quotes |
| the string `'a'×39`, `×40`, `×41`, `×45` under `%o` | `'aaaa…aaa…'`, 38 characters, then `…`, inside the quotes |
| the string `'a'×38`, `×39`, `×40` under `$field` | full, no quotes |
| the string `'a'×41`, `×45` under `$field` | `aaaa…aaa…`, 39 characters, then `…` |
| `'😀'×19` / `'😀'×20` under `%o` | full / `'😀…😀…'`, 19 emoji, then `…` |
| `'😀'×20` / `'😀'×21` under `$field` | full / `😀…😀…`, 19 emoji, then `…` |
| `[ '😀'×10, '😀'×10, 1 ]` under `%o` (33 characters, 53 units) | `[ '😀😀😀😀😀😀😀😀😀😀', …(2) ]` |
| `{ aaaaaaaaaaaaaaa: 1, bbbbbbbbbbbbbbbbbb: 2, ccccccccccccccc: 3 }` under `%s` | `{ aaaaaaaaaaaaaaa: 1, …(2) }` |

**What this analyzer does with that:** a string within the budget
(`taskTitleValueFormatTruncate`, 40 by default) renders in full, as the measured `×38` to `×40` and
`'😀'×20` rows do; a string past it is not proven and stays unrendered, as are composites under
`%o`, `%s` of a plain object and the tagged-template row's assembled object.

A rendered title is therefore a claim about the runtime string, and title comparison never decides
a finding on its own: the body and context still see the full values.

`[v5 CHANGE]` "Test Titles and Inspected Values Use `pretty-format`", in v5, interpolated string
values are **no longer wrapped in quotes** in the rendered title, and a `taskTitleValueFormatTruncate`
option (default 40 chars) truncates long interpolated values. This changes the exact title string
an extractor reproduces vs. pre-v5 Vitest, **the title-rendering logic must be versioned** (or the
extractor must not claim byte-exact title reproduction across Vitest majors, only per-major).

Back to the [Vitest surface index](../vitest-surface.md).
