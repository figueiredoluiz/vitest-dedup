// RECALL control -- duplicate-test present, shape: the twin of `hook-sibling-suite-write`, one edit
// away: the writer suite declares its own `ready`.
//
// The hook now writes a different binding, so both readers read the outer 0 in any order and
// remain duplicates. Pins that the refusal follows the binding, not its name.
import { describe, test, expect, beforeEach } from 'vitest'

let ready = 0

test('before', () => {
  expect(ready).toBe(0)
})

describe('writer', () => {
  let ready = 5
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
