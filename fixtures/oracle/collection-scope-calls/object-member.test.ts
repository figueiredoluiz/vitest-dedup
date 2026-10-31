// A call of an imported object's member.
import { describe, expect, test } from 'vitest'
import { api } from './support'

describe('object-member', () => {
  api.cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
