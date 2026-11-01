import { expect, test } from 'vitest'

test('binds one', () => {
  const [value] = [1]
})

test('binds two', () => {
  const [value] = [2]
})

test('nests one', () => {
  function unused() {
    const value = 1
  }
})

test('nests two', () => {
  function unused() {
    const value = 2
  }
})

test('reads one', () => {
  const [value] = [1]
  expect(value).toBeDefined()
})

test('reads two', () => {
  const [value] = [2]
  expect(value).toBeDefined()
})
