// RECALL negative -- benchmarks that are not duplicates, on Vitest 4.
//
// `sorts numbers` is a benchmark and `sorts numbers once` a test with the same body: Vitest runs
// one as a measurement and the other as a test, so they never pair. `copies an array` and
// `copies an array again` measure the same body with different `iterations`, which changes the
// measurement.
//
// Load-bearing: give `copies an array again` the same options and the pair becomes one
// `duplicate-test`.
import { bench, expect, test } from 'vitest'

bench('sorts numbers', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})

test('sorts numbers once', () => {
  expect([3, 1, 2].sort()).toEqual([1, 2, 3])
})

bench('copies an array', () => {
  ;[1, 2, 3].slice()
}, { iterations: 10 })

bench('copies an array again', () => {
  ;[1, 2, 3].slice()
}, { iterations: 20 })
