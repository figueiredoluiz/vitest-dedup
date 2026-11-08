// `after` reads the 1 that another suite's `beforeEach` left: the write is outside its lifecycle.
import { beforeEach, describe, expect, test } from 'vitest'

let ready = 0

test('before', () => {
  expect(ready).toBe(0)
})

describe('writer', () => {
  beforeEach(() => {
    ready = 1
  })
  test('writes', () => {
    expect(1).toBe(1)
  })
})

test('after', () => {
  expect(ready).toBe(1)
})
