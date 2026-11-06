// RECALL positive -- cross-file-duplicate between benchmarks on Vitest 4.
//
// `decode.bench.ts` measures exactly this. Load-bearing: change either body and the finding goes.
import { bench } from 'vitest'

bench('encodes a payload', () => {
  JSON.stringify({ id: 1, tags: ['a', 'b'] })
})
