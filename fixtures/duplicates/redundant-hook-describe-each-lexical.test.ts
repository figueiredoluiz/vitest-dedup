// A `describe.each` whose rows normalization proves a bounded population is an ordinary
// lifecycle scope: each generated suite registers the hooks its callback spells, in source
// order, before its tests (measured on Vitest 5.0.5 -- the hooks run once per generated suite,
// first then second, before that suite's tests). Two identical constant-write `beforeEach`
// declarations inside the callback therefore repeat work within every generated suite, and the
// pair is reported once by declaration location -- not once per generated suite, whatever the
// row count. The rows must be provable: the unread-table twin
// (not-redundant-hook-describe-each-unread.test.ts) changes only the rows argument and must
// stay silent.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=redundant-hook verdict=redundant severity=warning
// @vitest-dedup:participant L19 "beforeEach"
// @vitest-dedup:participant L22 "beforeEach"
import { describe, test, expect, beforeEach } from 'vitest'

let value = 0

describe.each([1, 2])('row %s', () => {
  beforeEach(() => {
    value = 1
  })
  beforeEach(() => {
    value = 1
  })

  test('reads the value', () => {
    expect(value).toBe(1)
  })
})
