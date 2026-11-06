// RECALL negative -- a member Vitest 4's `bench` does not have.
//
// `bench.concurrent` is undefined in Vitest 4: the call throws while the file loads, so no
// benchmark here registers and the equal pair below is never compared.
//
// Load-bearing: write `bench.skip` for `bench.concurrent` and the pair becomes one `duplicate-test`.
import { bench } from 'vitest'

bench.concurrent('sorts in parallel', () => {
  ;[3, 1, 2].sort()
})

bench('sorts numbers', () => {
  ;[3, 1, 2].sort()
})

bench('sorts numbers again', () => {
  ;[3, 1, 2].sort()
})
