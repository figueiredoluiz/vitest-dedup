// Vitest takes the first star holding a name: both stars hold `test`, and the API's is first.
import { beforeEach, expect, test } from './star-first'

beforeEach(() => {})
test('reads as a test', () => {
  expect(1).toBe(1)
})
