// RECALL variant -- duplicate-test absent, shape: a loop over a named array registers a twin.
//
// Vitest registers `loop` once per element of `rows`, but the analyzer does not prove how many
// elements a named array holds, so it leaves `loop` uncompared and reports the run incomplete.
import { test, expect } from 'vitest'

const rows = [1, 2]

for (const row of rows) {
  test('loop', () => {
    expect(1).toBe(1)
  })
}

test('twin', () => {
  expect(1).toBe(1)
})
