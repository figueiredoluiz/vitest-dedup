// A test function's `describe` and `suite` are the top-level suite: `skip` and `only` chained
// before them do not apply, so every child runs.
import { test, expect } from 'vitest'

test.describe('group', () => {
  test('one', () => { expect(1).toBe(1) })
})
test.skip.suite('other', () => {
  test('two', () => { expect(2).toBe(2) })
})
test.only.describe('not focused', () => {
  test('three', () => { expect(3).toBe(3) })
})

test('four', () => { expect(4).toBe(4) })
