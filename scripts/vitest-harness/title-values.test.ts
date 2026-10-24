// The differential-title fixture (V4): every `.each` row renders ONE title value through the
// directive under test, and `scripts/check/verify-titles.mjs` asserts the analyzer's rendered
// titles byte-match what Vitest itself prints at runtime. This is the permanent version of the
// ad-hoc probe that produced `docs/vitest-surface.md` §4's measurements: when either side
// changes -- a Vitest upgrade, a renderer edit -- the diff re-runs here instead of from memory.
//
// Values are inline literals the analyzer can statically prove, one per row, so every mismatch
// names a single rendering decision. Deliberately EXCLUDED shapes, with reasons:
//
// - composites nested past 8 levels: `MAX_CELL_DEPTH` refuses them outright (§4's "a refusal,
//   not a rendering difference"); there is no runtime title to compare against a refusal.
// - `%j` of composites over the budget: JSON-path truncation is unmeasured; short values only.
//
// Assertions are trivial on purpose: the tests must run and report titles; what is under test
// is the title text, not the bodies.
import { expect, test } from 'vitest'

// Scalar and array shapes across the budget boundary (§4's measured table).
test.each([
  [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]],
  [[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]],
  [[90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100]],
  [[{ aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa: 1 }]],
  [[{ a: { a: { a: { a: { a: { a: 1 } } } } } }]],
  [[{ a: { a: { a: { a: { a: 1 } } } } }]],
  // The quoted-string edge: 38 content characters quote to exactly 40 and stay whole; 39 quote
  // to 41 and are cut to 38, as are 40, 41 and 45. A content-length threshold passes 39 and 40
  // unchanged.
  [['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']],
  [['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']],
  [['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']],
  [['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']],
  [['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']],
  // UTF-16: 19 emoji quote to 40 units and stay whole; 20 are cut (to 19). Counted as scalars,
  // neither row crosses the budget.
  [['😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀']],
  [['😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀']],
  // Elision measured in units too: 33 characters, 53 units, one item fits.
  [[['😀😀😀😀😀😀😀😀😀😀', '😀😀😀😀😀😀😀😀😀😀', 1]]],
  [['short']],
  [[42]],
  [[3.5]],
  [[true]],
  [[null]],
])('inspect %o', () => {
  expect(true).toBe(true)
})

// `%s`: strings render bare, objects render as the inspect form (measured in the T6
// correction). The long bare string is INCLUDED deliberately: the harness is what measures
// whether the runner truncates it, and a mismatch here names the mechanism precisely.
test.each([
  ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'],
  ['short'],
  [42],
  [{ a: 1, b: 'two' }],
  // `%s` of a plain object goes through the same budgeted inspect as `%o` (the T6 correction).
  [{ aaaaaaaaaaaaaaa: 1, bbbbbbbbbbbbbbbbbb: 2, ccccccccccccccc: 3 }],
])('coerce %s', () => {
  expect(true).toBe(true)
})

// `%j`: short values only -- JSON-path truncation over the budget is unmeasured (see above).
test.each([
  [[{ a: 1 }]],
  [['short']],
  [[42]],
])('json %j', () => {
  expect(true).toBe(true)
})

// The named `$field` mechanism: same budget, different interpolation path.
test.each([
  { v: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9] },
  // The bare-string edge (`truncateString(value, 40)`): 38, 39 and 40 stay whole, 41 and 45
  // keep 39. A different cut from the quoted one above, so both edges are pinned.
  { v: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' },
  { v: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' },
  { v: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' },
  { v: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' },
  { v: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' },
  // 20 emoji are 40 units and stay whole; 21 are cut, and the cut backs off the split pair.
  { v: '😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀' },
  { v: '😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀' },
  { v: { a: { a: { a: { a: { a: { a: 1 } } } } } } },
])('named $v', () => {
  expect(true).toBe(true)
})

// The tagged-template form: one bound object per row, assembled from the columns.
test.each`
  a       | b
  ${1}    | ${'s'}
  ${2}    | ${'t'}
`('tagged %o', () => {
  expect(true).toBe(true)
})

// Global primitive values use distinct coercion, inspection, and JSON paths.
const ABSENT = undefined
const NOT_A_NUMBER = NaN
const UNBOUNDED = Infinity

test.each([[undefined], [ABSENT], [NaN], [NOT_A_NUMBER], [Infinity], [-UNBOUNDED], [[ABSENT, null, NaN, Infinity]]])('global coerce %s', () => {
  expect(true).toBe(true)
})
test.each([[undefined], [NaN], [Infinity], [[ABSENT, NaN]], [{ a: ABSENT, b: Infinity }]])('global inspect %o', () => {
  expect(true).toBe(true)
})
test.each([[NaN], [Infinity], [-Infinity], [[ABSENT, NaN]], [{ a: ABSENT, b: Infinity }]])('global json %j', () => {
  expect(true).toBe(true)
})
test.each([[Infinity], [-Infinity]])('global numeric %d', () => {
  expect(true).toBe(true)
})
test.each([{ v: undefined }, { v: NaN }, { v: Infinity }])('global field $v', () => {
  expect(true).toBe(true)
})
test.for([undefined, NaN, Infinity])('global for %s', () => {
  expect(true).toBe(true)
})
test.each`
  v
  ${undefined}
  ${NaN}
  ${Infinity}
`('global tagged $v', () => {
  expect(true).toBe(true)
})

// Numeric folds preserve signed zero in printf; string folds keep their own bytes.
test.each([[1 / -Infinity], [-1 / Infinity], [-0], ['-0' + ' ']])('global signed %s', () => {
  expect(true).toBe(true)
})
