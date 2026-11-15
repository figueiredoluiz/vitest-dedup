// RECALL variant -- duplicate-test absent, shape: a loop over a named array the file changes.
//
// Vitest registers `loop` once per element of `rows`, three after the `push`, but the analyzer
// does not follow what a call does to an array, so it leaves `loop` uncompared and reports the run
// incomplete. Without the `push`, `rows` is only iterated and `loop` pairs with `twin`.
import { test, expect } from 'vitest'

const rows = [1, 2]
rows.push(3)

for (const row of rows) {
  test('loop', () => {
    expect(1).toBe(1)
  })
}

test('twin', () => {
  expect(1).toBe(1)
})
