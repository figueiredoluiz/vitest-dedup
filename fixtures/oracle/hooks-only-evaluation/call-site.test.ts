// A call's arguments and substitutions are evaluated first, and may register tests.
import { describe, expect, test } from 'vitest'
import { hooks } from './helpers'
import * as support from './support'

describe('call site', () => {
  hooks()
  hooks(support.cases())
  hooks`${support.cases()}`
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
