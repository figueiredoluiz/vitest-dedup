// RECALL positive -- benchmarks, on a project that declares Vitest 4.
//
// Vitest 4 exports a top-level `bench` and collects `*.bench.ts` files in benchmark mode, so these
// are benchmarks it runs, compared with each other on the terms tests are:
//   - `parses a short document` / `reads a small config`: the same measurement twice;
//   - `formats a row` twice: one name, two different benchmarks;
//   - `measures a large config` twice: todos sharing a name;
//   - `splits lines` / `splits tokens`: the same measurement under two suites.
//
// Load-bearing: declare `"vitest": "^5.0.0"` in `package.json` and every finding goes, since
// Vitest 5 has no top-level `bench` and the file fails to load.
import { bench, describe } from 'vitest'

bench('parses a short document', () => {
  'a=1\nb=2\n'.split('\n').map((line) => line.split('='))
})

bench('reads a small config', () => {
  'a=1\nb=2\n'.split('\n').map((line) => line.split('='))
})

bench('formats a row', () => {
  ;[1, 2].join(',')
})

bench('formats a row', () => {
  new Set(['a']).has('a')
})

bench.todo('measures a large config')
bench.todo('measures a large config')

describe('parser', () => {
  bench('splits lines', () => {
    'x\ny'.split('\n').reverse()
  })
})

describe('lexer', () => {
  bench('splits tokens', () => {
    'x\ny'.split('\n').reverse()
  })
})
