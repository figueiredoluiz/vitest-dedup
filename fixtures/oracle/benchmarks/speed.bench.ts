// A benchmark Vitest's `list` does not collect: it is counted, one excluded case each, until compared.
import { bench, expect } from 'vitest'

bench('adds', () => {
  expect(1 + 1).toBe(2)
})
