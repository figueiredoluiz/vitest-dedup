// Second half of `cross-file-duplicate-bench-vitest4`: the same measurement as `encode.bench.ts`.
import { bench } from 'vitest'

bench('serializes a payload', () => {
  JSON.stringify({ id: 1, tags: ['a', 'b'] })
})
