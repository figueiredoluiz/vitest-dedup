// Twin of redundant-hook-before-all-invariant.test.ts with `afterAll` in both scopes: the
// outer `afterAll` runs after every suite, so the nested write is the first establishment of
// the value in its window -- deleting it delays the write past the suites that run in between,
// which those suites can observe. Named weakening: change both phases to `beforeAll` and the
// pair fires exactly as in the twin.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:participant L15 "afterAll"
// @vitest-dedup:participant L20 "afterAll"
// @vitest-dedup:isolates not-isolable the two hooks are byte-identical, so the isolated variable is the phase-scope relation (once-per-suite deletion order), which no participant fingerprint carries; the guard is the corpus gate's silence assertion plus the named weakening in the header
import { describe, test, expect, afterAll } from 'vitest'

let ready = 0

afterAll(() => {
  ready = 1
})

describe('inner', () => {
  afterAll(() => {
    ready = 1
  })

  test('runs before cleanup', () => {
    expect(ready).toBe(0)
  })
})
