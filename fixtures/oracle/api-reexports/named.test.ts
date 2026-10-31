// A name re-exported from the API's module is the API, renamed or not.
import { beforeEach, check, expect, test } from './named'

beforeEach(() => {})
test('reads as a test', () => {
  expect(1).toBe(1)
})
check('reads under a rename', () => {
  expect(2).toBe(2)
})
