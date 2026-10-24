// Only a test body reads the missing module, and test bodies do not run: this file is compared.
import { expect, test } from 'vitest'
import { add } from './missing-module'

test('adds', () => {
  expect(add(1, 2)).toBe(3)
})
