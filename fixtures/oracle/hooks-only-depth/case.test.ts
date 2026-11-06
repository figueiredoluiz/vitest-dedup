// Past the import depth limit, imported effects and registrations remain unknown.
import { describe, expect, test } from 'vitest'
import { f0 } from './m0'

describe('depth', () => {
  f0()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
