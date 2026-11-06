// Runtime control for the cross-scope `beforeAll` claim: the outer hook runs before the nested
// suite's hooks and tests, so the nested constant write re-establishes state the outer already
// established. Removing either hook keeps the test passing; the analyzer reports the pair.
import { describe, test, expect, beforeAll } from 'vitest'

let ready = 0

beforeAll(() => {
  ready = 1
})

describe('inner', () => {
  beforeAll(() => {
    ready = 1
  })

  test('is ready', () => {
    expect(ready).toBe(1)
  })
})
