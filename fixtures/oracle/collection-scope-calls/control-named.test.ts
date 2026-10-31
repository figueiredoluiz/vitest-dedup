// Control: a named import called directly is a wrapper.
import { describe, expect, test } from 'vitest'
import { cases } from './support'

describe('control-named', () => {
  cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
