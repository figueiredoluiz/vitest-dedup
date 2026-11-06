// RECALL negative -- top-level `bench` on a project that declares Vitest 5.
//
// Vitest 5 has no top-level `bench` (benchmarks run through `test`'s `bench` fixture), so this file
// fails to load and its two declarations register nothing. They stay excluded, never paired.
//
// Load-bearing: declare `"vitest": "4.1.12"` in `package.json` and the pair becomes one
// `duplicate-test`, as in `duplicate-test-bench-vitest4`.
import { bench } from 'vitest'

bench('parses a short document', () => {
  'a=1\nb=2\n'.split('\n').map((line) => line.split('='))
})

bench('reads a small config', () => {
  'a=1\nb=2\n'.split('\n').map((line) => line.split('='))
})
