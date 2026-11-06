// A benchmark Vitest's `list` does not collect. The directory declares no Vitest version, so whether a
// top-level `bench` exists is unknown: it is counted, one excluded case, never compared.
import { bench, expect } from 'vitest'

bench('adds', () => {
  expect(1 + 1).toBe(2)
})
