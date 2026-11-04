// A hook registered through a test function's member is the top-level hook: `skip` chained before
// it does not change it.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=redundant-hook verdict=redundant severity=warning
// @vitest-dedup:participant L11 "beforeEach"
// @vitest-dedup:participant L12 "beforeEach"
import { beforeEach, expect, test } from 'vitest'

let value = 0

beforeEach(() => { value = 1 })
test.skip.beforeEach(() => { value = 1 })

test('reads the value', () => { expect(value).toBe(1) })
