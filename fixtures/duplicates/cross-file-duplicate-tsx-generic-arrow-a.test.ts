// Pair with cross-file-duplicate-tsx-generic-arrow-b.test.ts: identical tests calling the same
// imported helper. The `-b` side also contains a generic arrow function `<T>(value: T) => value`,
// valid in `.ts` but not in TSX. The import map is read from each file's own parse, so both sides
// resolve `tsxImportSum` to the same import and the pair is a cross-file duplicate. Named
// weakening: build the map from a TSX re-parse again and `-b`'s map is empty, its reference falls
// back to a name identity, and the finding disappears.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=cross-file-duplicate verdict=redundant severity=warning
// @vitest-dedup:participant L14 "sums two numbers through a generic-arrow file"
// @vitest-dedup:participant cross-file-duplicate-tsx-generic-arrow-b.test.ts#L11 "sums two numbers through a generic-arrow file"
import { test, expect } from 'vitest'
import { tsxImportSum } from './tsx-import-sum'

test('sums two numbers through a generic-arrow file', () => {
  expect(tsxImportSum(4, 5)).toBe(9)
})
