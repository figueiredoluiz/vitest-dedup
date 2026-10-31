// Past the import depth limit a module reads as exporting nothing, so nothing above it is hooks-only.
import { describe, expect, test } from 'vitest'
import { f0 } from './m0'

describe('depth', () => {
  f0()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
