// A star from a package the analyzer cannot read may hold the name, so it is refused.
import { beforeEach, expect, test } from './star-package'

beforeEach(() => {})
test('reads as a test', () => {
  expect(1).toBe(1)
})
