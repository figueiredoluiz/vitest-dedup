// RECALL negative -- benchmarks reading state a hook resets, on Vitest 4.
//
// Vitest 4 runs no hook around a benchmark: the `beforeEach` below never resets `count`, so the
// second benchmark starts from what the first left behind. Both read state another benchmark
// writes, so neither is compared, though their bodies are equal.
//
// Load-bearing: write `test` for `bench` in both declarations and the analyzer counts the reset,
// since a hook does run before each test, and compares the pair.
import { beforeEach, bench } from 'vitest'

let count = 0

beforeEach(() => {
  count = 0
})

bench('counts up', () => {
  count += 1
  String(count)
})

bench('counts again', () => {
  count += 1
  String(count)
})
