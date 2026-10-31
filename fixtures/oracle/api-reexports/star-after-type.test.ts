// A type-only star is erased, so it is no star before the API's.
import { beforeEach, expect, test } from './star-after-type'

beforeEach(() => {})
test('reads as a test', () => {
  expect(1).toBe(1)
})
