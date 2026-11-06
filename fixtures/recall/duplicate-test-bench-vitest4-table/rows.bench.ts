// RECALL negative -- a benchmark table on Vitest 4.
//
// Vitest 4 has no `bench.each`: the call throws while the file loads, empty table or not, so no
// benchmark in this file registers. The two equal benchmarks below are never compared.
//
// Load-bearing: delete the `bench.each` declaration and the pair becomes one `duplicate-test`.
import { bench } from 'vitest'

bench.each([])('row %i', () => {
  ;[1, 2].map((x) => x * 2)
})

bench('doubles numbers', () => {
  ;[1, 2].map((x) => x * 2)
})

bench('doubles numbers again', () => {
  ;[1, 2].map((x) => x * 2)
})
