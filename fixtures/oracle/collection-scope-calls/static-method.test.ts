// A class's static method.
import { describe, expect, test } from 'vitest'
import { Cases } from './support'

describe('static-method', () => {
  Cases.run()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
