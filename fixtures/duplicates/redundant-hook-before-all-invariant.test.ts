// A nested `beforeAll` whose body repeats a constant write the file-scope `beforeAll` already
// performs: the outer hook runs before every descendant suite's hooks and tests, and the
// module-wide write set proves no code can change the cell in between, so the nested
// registration is redundant. The phase twin
// (not-redundant-hook-after-all-cross-scope.test.ts) flips both phases and must stay silent:
// the outer `afterAll` runs last, so the nested write is the first establishment of the value
// in its window and deleting it is observable to the suites that run before it.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=redundant-hook verdict=redundant severity=warning
// @vitest-dedup:participant L16 "beforeAll"
// @vitest-dedup:participant L21 "beforeAll"
import { describe, test, expect, beforeAll } from 'vitest'

let ready = 0

beforeAll(() => {
  ready = 1
})

describe('inner', () => {
  beforeAll(() => {
    ready = 1
  })

  test('is ready', () => {
    expect(ready).toBe(1)
  })
})
