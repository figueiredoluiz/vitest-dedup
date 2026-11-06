// RECALL positive -- duplicate-test-name.
//
// Rows of tables with nothing to compare render their titles like any row. `rounds 1` comes from an
// empty-callback table and from a todo table; `formats a` from an empty-callback row and a written
// test. Each is one finding.
//
// The repeated row `repeats 4` belongs to one declaration, so it is not a name collision: two views
// of one call are `duplicate-each-row`'s question, which empty rows never reach.
//
// Load-bearing: change `[1, 3]` to `[3]` and the `rounds 1` finding goes.
import { expect, test } from 'vitest'

test.each([1, 2])('rounds %i', () => {})
test.each([1, 3])('rounds %i')

test.each(['a'])('formats %s', () => {})
test('formats a', () => {
  expect('a').toHaveLength(1)
})

test.each([4, 4])('repeats %i', () => {})
