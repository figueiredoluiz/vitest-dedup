// RECALL control -- duplicate-test present, shape: the twin of `hook-after-each-restore` with a
// signed sentinel and an explicit `undefined` as the declared values.
//
// Each `afterEach` writes back the value its binding is declared with, so every test starts from it
// and each suite's readers remain duplicates.
import { describe, test, expect, afterEach } from 'vitest'

describe('sentinel', () => {
  let index = -1
  afterEach(() => {
    index = -1
  })
  test('a', () => {
    expect(index).toBe(-1)
  })
  test('b', () => {
    expect(index).toBe(-1)
  })
})

describe('unset', () => {
  let current = undefined
  afterEach(() => {
    current = undefined
  })
  test('a', () => {
    expect(current).toBeUndefined()
  })
  test('b', () => {
    expect(current).toBeUndefined()
  })
})
