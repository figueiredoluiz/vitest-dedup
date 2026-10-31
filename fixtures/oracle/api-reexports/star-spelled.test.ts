// A name spelled beside the star is refused, so a call that registers nothing is no test.
import { expect, test as visible } from 'vitest'
import { beforeEach, test } from './star-spelled'

beforeEach(() => {})
test('never registered', () => {})
visible('stays comparable', () => {
  expect(1).toBe(1)
})
