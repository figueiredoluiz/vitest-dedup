// Pair with cross-file-duplicate-tsx-legacy-cast-b.test.ts: identical tests calling the same
// imported helper. The `-b` side also contains a legacy `<T>value` type cast, valid in `.ts` but
// not as TSX. The import map is parsed as TSX, so `-b`'s map is empty, `tsxImportSum` falls back to
// a name identity, and the real cross-file duplicate is not reported (M12). Named weakening: delete
// that line and the finding fires.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=cross-file-duplicate verdict=redundant severity=warning
// @vitest-dedup:pending M12: the import map is parsed as TSX, which rejects this pair's -b side
// @vitest-dedup:participant L14 "sums two numbers through a cast-carrying file"
// @vitest-dedup:participant cross-file-duplicate-tsx-legacy-cast-b.test.ts#L12 "sums two numbers through a cast-carrying file"
import { test, expect } from 'vitest'
import { tsxImportSum } from './tsx-import-sum'

test('sums two numbers through a cast-carrying file', () => {
  expect(tsxImportSum(2, 3)).toBe(5)
})
