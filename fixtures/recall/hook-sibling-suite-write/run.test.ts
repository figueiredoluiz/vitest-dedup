// RECALL control -- duplicate-test absent, shape: a sibling suite's `beforeEach` assigns a binding
// declared outside it, between two identical readers.
//
// The hook runs only around its own suite's test, but its write stays: `before` reads 0 and
// passes, `after` reads 1 and fails. A hook write counted only when it built on the previous
// value, so this plain assignment let the two readers be reported as error-severity duplicates.
// Both readers are now order dependent; the twin `hook-sibling-suite-shadowed` gives the writer
// its own binding.
import { describe, test, expect, beforeEach } from 'vitest'

let ready = 0

test('before', () => {
  expect(ready).toBe(0)
})

describe('writer', () => {
  beforeEach(() => {
    ready = 1
  })
  test('writes', () => {
    expect(true).toBe(true)
  })
})

test('after', () => {
  expect(ready).toBe(0)
})
