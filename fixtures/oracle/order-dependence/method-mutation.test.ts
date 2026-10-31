// `reads` passes only after `pushes` has run: the write is a method call on a shared object.
import { expect, test } from 'vitest'

const list: number[] = []

test('pushes', () => {
  list.push(1)
  expect(1).toBe(1)
})

test('reads', () => {
  expect(list).toHaveLength(1)
})

test('stays comparable', () => {
  expect(1).toBe(1)
})
