import { expect, test } from 'vitest'

const options = () => ({ repeats: 2 })

test('later spread decides the count', { repeats: 0, ...options() }, () => {
  expect(1 + 2).toBe(3)
})

test('getter decides the count', { get repeats() { return 2 } }, () => {
  expect(1 + 2).toBe(3)
})

test('later count wins', { ...options(), repeats: 0 }, () => {
  expect(1 + 2).toBe(3)
})

test('later computed count wins', { ...options(), ['repeats']: 0 }, () => {
  expect(1 + 2).toBe(3)
})
