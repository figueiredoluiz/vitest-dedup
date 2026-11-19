import { expect, test } from 'vitest'
import { condition, left, sequence, space, label, count } from './values'
import named from './named'
condition('condition', () => expect(1).toBe(1))
left('left', () => expect(1).toBe(1))
sequence('sequence', () => expect(1).toBe(1))
space.expect(1).toBe(1)
label.toUpperCase()
count.toFixed()
named.toUpperCase()
test('first', () => {
  expect([1, 2].length).toBe(2)
})
test('second', () => {
  expect([1, 2].length).toBe(2)
})
