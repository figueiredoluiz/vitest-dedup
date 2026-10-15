// Pair with cross-file-duplicate-tsx-legacy-cast-a.test.ts; see that file.
// The line below the imports is the only difference.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=cross-file-duplicate verdict=redundant severity=warning
// @vitest-dedup:pending M12: the import map is parsed as TSX, which rejects this file
// @vitest-dedup:participant cross-file-duplicate-tsx-legacy-cast-a.test.ts#L14 "sums two numbers through a cast-carrying file"
// @vitest-dedup:participant L12 "sums two numbers through a cast-carrying file"
import { test, expect } from 'vitest'
import { tsxImportSum } from './tsx-import-sum'
const castValue = <number>(1 as unknown)

test('sums two numbers through a cast-carrying file', () => {
  expect(tsxImportSum(2, 3)).toBe(5)
})
