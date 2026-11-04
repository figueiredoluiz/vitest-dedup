// RECALL negative -- duplicate-test-name, control for duplicate-test-long-row-names: two names
// that differ only past the 40th UTF-16 unit. Vitest 5.0.5 displays both as the same 39 units and
// `…` under its default `taskTitleValueFormatTruncate`, but a project may set that limit (`0`
// keeps them distinct), so a cut value is display text, never a name to compare.
import { expect, test } from 'vitest'

test.each([{ name: 'checks the limit of a value long enough to be cut: first' }])('$name', () => {
  expect([1]).toHaveLength(1)
})

test.each([{ name: 'checks the limit of a value long enough to be cut: second' }])('$name', () => {
  expect('a').toMatch(/a/)
})
