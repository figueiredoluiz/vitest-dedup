// RECALL positive -- duplicate-test expected. Creating a registration factory registers no test; the real pair remains a duplicate.
import { test, describe, expect } from 'vitest'
const register = () => {
  const each = test.each([1, 2])
  const rows = test.for([1, 2])
  const skipped = test.skipIf(true)
  const enabled = test.runIf(true)
  const suites = describe.each([1, 2])
  const chained = test.skipIf(false).each([1, 2])
  const tagged = test.each`value
    ${1}
    ${2}
  `
  test('first', () => { expect(1).toBe(1) })
  test('second', () => { expect(1).toBe(1) })
}
register()
