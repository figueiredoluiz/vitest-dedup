// A type annotation on a binding is not a second declaration.
import { describe, expect, test } from 'vitest'
import { annotated, closed } from './annotated-binding'

describe('annotated-binding', () => {
  closed()
  annotated()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
