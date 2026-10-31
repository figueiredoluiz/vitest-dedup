// A call of a member of a default export.
import { describe, expect, test } from 'vitest'
import api from './support'

describe('default-object', () => {
  api.cases()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
