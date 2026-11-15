// RECALL variant -- duplicate-test absent, shape: a generator as the `forEach` callback.
//
// Calling a generator only creates an iterator, so Vitest never runs its body and registers no
// `loop`; the analyzer leaves `loop` uncompared and reports the run incomplete. Load-bearing:
// drop the `*` and `loop` runs once per element and pairs with `twin`.
import { test, expect } from 'vitest'

const rows = [1, 2]

;[1, 2].forEach(function* () {
  test('loop', () => {
    expect(1).toBe(1)
  })
})

rows.forEach(function* () {
  test('named', () => {
    expect(1).toBe(1)
  })
})

test('twin', () => {
  expect(1).toBe(1)
})
