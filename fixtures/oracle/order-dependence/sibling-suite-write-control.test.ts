// Control: the writer suite has its own `ready`. One edit from `sibling-suite-write`.
import { beforeEach, describe, expect, test } from 'vitest'

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
    expect(ready).toBe(1)
  })
})

test('after', () => {
  expect(ready).toBe(0)
})
