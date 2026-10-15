// Two tests calling different helpers whose inlined bodies are identical: both wrap the same
// imported `tsxImportNormalize`. `tsx-import-wrap-two.ts` also contains a legacy `<T>value`
// cast. Helper files' import maps are parsed as TSX, so that helper's map is empty, its
// reference falls back to a name identity, and the duplicate is not reported. Named
// weakening: delete the cast line from the helper and the finding fires.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:pending helper files' import maps are parsed as TSX, which rejects tsx-import-wrap-two.ts
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
