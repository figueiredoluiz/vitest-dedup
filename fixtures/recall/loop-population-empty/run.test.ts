// RECALL negative -- duplicate-test absent, shape: a loop over an empty array encloses a twin.
//
// Vitest iterates nothing, so neither `never` nor the benchmark is registered and only `twin`
// runs. Iterating `[1]` instead registers `never` once and the pair is a duplicate.
import { bench, test, expect } from 'vitest'

for (const row of []) {
  bench('never', () => {})
  test('never', () => {
    expect(1).toBe(1)
  })
}

test('twin', () => {
  expect(1).toBe(1)
})
