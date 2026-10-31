// A suite title computed by a call that registers.
import { describe, expect, test } from 'vitest'
import { title } from './support'

describe(title(), () => {
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
