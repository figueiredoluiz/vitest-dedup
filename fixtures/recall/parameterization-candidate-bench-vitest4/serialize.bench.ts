// RECALL positive -- parameterization-candidate between benchmarks, on Vitest 4.
//
// Two benchmarks with the same structure, differing only in one literal. Every case is needed;
// the finding's advice for benchmarks is one loop of `bench(...)` calls, since Vitest 4 has no
// `bench.each` and a `test.each` table would not be measured.
//
// Load-bearing: make the second literal `1` and the pair becomes a `duplicate-test` instead.
import { bench } from 'vitest'

bench('serializes a small object', () => {
  JSON.stringify({ size: 1 })
})

bench('serializes a larger object', () => {
  JSON.stringify({ size: 2 })
})
