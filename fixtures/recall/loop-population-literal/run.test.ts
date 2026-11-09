// RECALL positive -- duplicate-test expected, shape: a loop over a literal array registers a twin.
//
// Vitest registers `loop` once per element; each iteration repeats `twin`. The pair is reported
// once for the shared declaration, and the quality gate counts both iterations.
import { test, expect } from 'vitest'

for (const row of [1, 2]) {
  test('loop', () => {
    expect(1).toBe(1)
  })
}

test('twin', () => {
  expect(1).toBe(1)
})
