// Two tests calling different helpers whose inlined bodies are identical: both wrap the same
// imported `tsxImportNormalize`. `tsx-import-wrap-two.ts` also contains a legacy `<T>value` cast,
// valid in `.ts` but not in TSX. Each helper's import map is read from its own parse, so both
// references resolve to the same import and the tests are duplicates. Named weakening: build the
// helper's map from a TSX re-parse again and that helper's map is empty, its reference falls back
// to a name identity, and the finding disappears.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L15 "normalizes through the first wrapper"
// @vitest-dedup:participant L19 "normalizes through the second wrapper"
import { test, expect } from 'vitest'
import { tsxImportWrapOne } from './tsx-import-wrap-one'
import { tsxImportWrapTwo } from './tsx-import-wrap-two'

test('normalizes through the first wrapper', () => {
  expect(tsxImportWrapOne(1)).toBe(2)
})

test('normalizes through the second wrapper', () => {
  expect(tsxImportWrapTwo(1)).toBe(2)
})
