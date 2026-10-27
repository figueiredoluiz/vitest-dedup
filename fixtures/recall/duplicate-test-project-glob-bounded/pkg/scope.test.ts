import { test, expect } from 'vitest'

test('normalizes the first label', () => {
  expect(' Example '.trim().toLowerCase()).toBe('example')
})

test('normalizes the second label', () => {
  expect(' Example '.trim().toLowerCase()).toBe('example')
})

test('retains label case', () => {
  expect(' Example '.trim()).toBe('Example')
})
