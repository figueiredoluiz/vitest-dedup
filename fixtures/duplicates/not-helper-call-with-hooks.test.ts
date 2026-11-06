// Two twin suites each calling a helper that registers a `beforeEach` and a test. A hook
// registered inside a suite governs every test there, handwritten ones included, and the engine
// composes hook context only file-wide for expanded tests, so a suite-position call whose
// invocation registers a hook refuses: the whole file's expansion keeps today's exclusions.
// Named weakening: delete the `beforeEach` from `install` and the two calls expand and pair.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates not-isolable the isolated variable is the hook the invocation registers, which no participant fingerprint carries; the guard is the corpus gate's silence assertion plus the named weakening in the header
// @vitest-dedup:participant L26 "reads"
// @vitest-dedup:participant L30 "reads"
import { describe, test, expect, beforeEach } from 'vitest'

let ready = 0

/** Registers the suite's `beforeEach` and its reading test, titled from the call's label. */
function install(label: string) {
  beforeEach(() => {
    ready = 1
  })
  test(`${label}`, () => {
    expect(ready).toBe(1)
  })
}

describe('a', () => {
  install('reads')
})

describe('b', () => {
  install('reads')
})
