// A member of a hooks-only helper is any value: read or destructured, it may register tests.
import { describe, expect, test } from 'vitest'
import { setup } from './setup'

const { test: destructured } = setup

describe('member', () => {
  setup()
  setup.test('through a member', () => {})
  destructured('through a destructured member', () => {})
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
