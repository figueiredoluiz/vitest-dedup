// RECALL negative -- `bench` declarations in a test file.
//
// The project declares Vitest 4, which collects `*.bench.ts` files in benchmark mode only. This is
// a test file: `vitest` refuses a `bench()` call outside benchmark mode ("`bench()` is only
// available in benchmark mode") and loads nothing from it, and `vitest bench` does not collect it.
// So these two never run, and are never paired, though their bodies are equal. `perf.bench.ts`
// holds the same pair where it does run.
import { bench } from 'vitest'
import { parse } from './parser'

bench('parses a short document', () => {
  const source = 'a=1\nb=2\n'
  parse(source)
})

bench('reads a small config', () => {
  const text = 'a=1\nb=2\n'
  parse(text)
})
