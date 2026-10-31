// What an imported wrapper registers is compared once wrappers are inlined.
import { describe, expect, test } from 'vitest'
import { cases } from './cases'

describe('wrapped', () => {
  cases()
  test('stays comparable', () => {
    expect(2).toBe(2)
  })
})
