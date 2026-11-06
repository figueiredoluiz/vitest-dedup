// RECALL positive -- duplicate-test over `bench` declarations, on a project that declares Vitest 4.
//
// A `*.bench.ts` file is collected by `vitest bench` through the separate `benchmark.include`
// channel, and Vitest 4 exports a top-level `bench`. Two benchmarks with equal bodies under one
// context measure the same thing twice, on exactly the terms two equal tests are redundant.
import { bench } from 'vitest'
import { parse } from './parser'

bench('parses a long document', () => {
  const source = 'x=1\ny=2\nz=3\n'
  parse(source)
})

bench('reads a large config', () => {
  const text = 'x=1\ny=2\nz=3\n'
  parse(text)
})
