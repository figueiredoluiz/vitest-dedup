// RECALL negative -- duplicate-test over `.each` rows whose callback writes its parameter.
//
// Both callbacks reassign the parameter before reading it, so the row's cell no longer describes
// what the reads observe. Substituting the cell into those reads would render `expect(6).toBe(6)`
// for both rows and merge them at Error severity, while Vitest evaluates the writes (5.0.5: the
// first fails `expected 5 to be 6`, the second `expected 6 to be 5` -- recorded on
// node v24.21.0, darwin-arm64). Row substitution is therefore declined for a written parameter,
// all-or-nothing, by both engines -- see `src/normal/rows.rs` and `resolve_row_substitution`.
//
// With the substitution declined, the rows stay unsubstituted and genuinely differ (the
// parameter read sits in the assertion's subject in one and its expected value in the other), so
// nothing may fire. The pairs `duplicate-each-row-positive` and
// `duplicate-each-row-const-identifier` prove the refusal is not substitution being switched
// off: readable, unwritten rows still substitute and still match.
import { expect, test } from 'vitest'

test.each([6])('reassigns the row then asserts it', (x) => {
  x = 5
  expect(x).toBe(6)
})

test.each([6])('asserts the literal against the row', (x) => {
  x = 5
  expect(6).toBe(x)
})
