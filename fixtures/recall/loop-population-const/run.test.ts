// RECALL positive -- duplicate-test expected, shape: a loop over a `const` array registers a twin.
//
// `rows` is read only as what the loops iterate, so its two elements are what Vitest counts:
// `loop` registers twice, each iteration repeats `twin`, and the gate counts both. `none` is
// empty, so `never` is not registered and is not compared with `twin`.
//
// Load-bearing: add `rows.push(3)` (or declare `rows` with `let`) and `loop` is left uncompared.
import { test, expect } from 'vitest'

const rows = [1, 2]
const none: number[] = []

for (const row of rows) {
  test('loop', () => {
    expect(1).toBe(1)
  })
}

none.forEach(() => {
  test('never', () => {
    expect(1).toBe(1)
  })
})

test('twin', () => {
  expect(1).toBe(1)
})
