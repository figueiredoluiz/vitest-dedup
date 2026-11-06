// RECALL negative -- top-level `bench` on a project whose `vitest` range spans two majors.
//
// `^4.0.0-beta.1 || ^5.0.0` admits Vitest 5, which has no top-level `bench`, so whether these
// register is not known: they stay excluded, never paired.
//
// Load-bearing: declare `"vitest": "4.1.12"` and the pair becomes one `duplicate-test`.
import { bench } from 'vitest'

bench('parses a short document', () => {
  'a=1\nb=2\n'.split('\n').map((line) => line.split('='))
})

bench('reads a small config', () => {
  'a=1\nb=2\n'.split('\n').map((line) => line.split('='))
})
