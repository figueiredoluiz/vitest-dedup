// Two twin suites each calling a helper that registers a `beforeEach` and a test: the helper's
// hook joins the caller suite's hook chain at the call's position (after any hook the suite body
// spells before it), and the hook's proven lexical write is part of the case context, so the
// suite pair still pairs. The hooks' cells keep one module-wide value -- the write-set proof the
// hook claim rests on. The helper's written test is not counted twice: it registers only
// through the two calls.
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L20 "a > reads"
// @vitest-dedup:participant L20 "b > reads"
import { describe, test, expect, beforeEach } from 'vitest'

let ready = 0

/** Registers the suite's `beforeEach` and its reading test. */
function install() {
  beforeEach(() => {
    ready = 1
  })
  test('reads', () => {
    expect(ready).toBe(1)
  })
}

describe('a', () => {
  install()
})

describe('b', () => {
  install()
})
