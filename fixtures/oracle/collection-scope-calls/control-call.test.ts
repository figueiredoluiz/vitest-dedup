// Control: `.call` keeps the callee a wrapper.
import { describe, expect, test } from 'vitest'
import { cases } from './support'

describe('control-call', () => {
  cases.call(null)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
