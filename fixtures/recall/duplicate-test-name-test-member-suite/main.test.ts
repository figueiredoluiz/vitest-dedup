// Tests inside a test function's `describe` are collected, and `skip` chained before it does not
// skip the suite. The top-level test of the same name has a different full name.
// @vitest-dedup:extracted 3
// @vitest-dedup:expect rule=duplicate-test-name verdict=duplicate severity=warning
// @vitest-dedup:participant L12 "parser > reads the header"
// @vitest-dedup:participant L13 "parser > reads the header"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:participant L16 "reads the header"
import { expect, test } from 'vitest'

test.skip.describe('parser', () => {
  test('reads the header', () => { expect('a').toBe('a') })
  test('reads the header', () => { expect(['b']).toHaveLength(1) })
})

test('reads the header', () => { expect({ c: 1 }).toEqual({ c: 1 }) })
