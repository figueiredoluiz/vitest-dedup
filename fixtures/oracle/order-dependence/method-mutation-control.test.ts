// Control: each test has its own list. One edit from `method-mutation`.
import { expect, test } from 'vitest'

test('pushes', () => {
  const list: number[] = []
  list.push(1)
  expect(1).toBe(1)
})

test('reads', () => {
  const list: number[] = [1]
  expect(list).toHaveLength(1)
})

test('stays comparable', () => {
  expect(1).toBe(1)
})
