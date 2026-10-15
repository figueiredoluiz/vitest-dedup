// Pair with cross-file-duplicate-tsx-generic-arrow-a.test.ts; see that file.
// The line below the imports is the only difference.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=cross-file-duplicate verdict=redundant severity=warning
// @vitest-dedup:participant cross-file-duplicate-tsx-generic-arrow-a.test.ts#L14 "sums two numbers through a generic-arrow file"
// @vitest-dedup:participant L11 "sums two numbers through a generic-arrow file"
import { test, expect } from 'vitest'
import { tsxImportSum } from './tsx-import-sum'
const identity = <T>(value: T): T => value

test('sums two numbers through a generic-arrow file', () => {
  expect(tsxImportSum(4, 5)).toBe(9)
})
