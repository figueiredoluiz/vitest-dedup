// A helper in the test file calling an export the cycle left out is not hooks-only.
import { beforeEach, describe, expect, test } from 'vitest'
import { only } from './a'
/** Registers a hook, then calls what the cycle left out. */
function setup() {
  beforeEach(() => {})
  only()
}

describe('local', () => {
  setup()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
