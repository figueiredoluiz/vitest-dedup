// RECALL control -- duplicate-test absent, shape: the twin of `hook-before-all-shield`: a suite's
// `beforeAll` assigns the binding, and a suite nested inside it writes it again between the readers.
//
// The `beforeAll` runs once, before `a`; the nested suite's `beforeEach` then leaves 1 for `b`:
// `a` passes and `b` fails. A `beforeAll` around the readers only protects them from writers
// outside its own suite.
import { describe, test, expect, beforeAll, beforeEach } from 'vitest'

let app = 0

describe('outer', () => {
  beforeAll(() => {
    app = 0
  })
  test('a', () => {
    expect(app).toBe(0)
  })
  describe('writer', () => {
    beforeEach(() => {
      app = 1
    })
    test('writes', () => {
      expect(true).toBe(true)
    })
  })
  test('b', () => {
    expect(app).toBe(0)
  })
})
