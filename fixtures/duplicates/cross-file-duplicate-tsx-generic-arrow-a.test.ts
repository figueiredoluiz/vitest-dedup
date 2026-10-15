// Pair with cross-file-duplicate-tsx-generic-arrow-b.test.ts: identical tests calling the same
// imported helper. The `-b` side also contains a generic arrow function `<T>(value: T) => value`,
// valid in `.ts` but not as TSX. The import map is parsed as TSX, so `-b`'s map is empty,
// `tsxImportSum` falls back to a name identity, and the real cross-file duplicate is not reported
// (M12). Named weakening: delete that line and the finding fires.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=cross-file-duplicate verdict=redundant severity=warning
// @vitest-dedup:pending M12: the import map is parsed as TSX, which rejects this pair's -b side
// @vitest-dedup:participant L14 "sums two numbers through a generic-arrow file"
// @vitest-dedup:participant cross-file-duplicate-tsx-generic-arrow-b.test.ts#L12 "sums two numbers through a generic-arrow file"
import { test, expect } from 'vitest'
import { tsxImportSum } from './tsx-import-sum'

test('sums two numbers through a generic-arrow file', () => {
  expect(tsxImportSum(4, 5)).toBe(9)
})
