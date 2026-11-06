// RECALL negative -- benchmarks whose `setup` option writes what they read, on Vitest 4.
//
// `setup` runs around each benchmark's measurement, so the second benchmark starts from what the
// first one's setup left in `count`: equal bodies observing different values. A benchmark whose
// options hold a callback is not compared.
//
// Load-bearing: drop the `setup` options and the pair becomes one `duplicate-test`.
import { bench } from 'vitest'

let count = 0

bench('reads the count', () => {
  String(count)
}, { setup: () => { count += 1 } })

bench('reads the count again', () => {
  String(count)
}, { setup: () => { count += 1 } })
