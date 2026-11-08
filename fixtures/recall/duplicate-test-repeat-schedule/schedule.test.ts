import { describe, expect, test } from 'vitest'

test('once', { retry: 2 }, ({ task }) => {
  console.log('attempt', task.name, task.result?.repeatCount)
  expect(task.result?.repeatCount).toBeGreaterThanOrEqual(0)
})

test('three passes', { retry: 2, repeats: 2 }, ({ task }) => {
  console.log('attempt', task.name, task.result?.repeatCount)
  expect(task.result?.repeatCount).toBeGreaterThanOrEqual(0)
})

describe('repeated suite', { repeats: 2 }, () => {
  test('inherits three passes', ({ task }) => {
    console.log('attempt', task.name, task.result?.repeatCount)
    expect(task.result?.repeatCount).toBeGreaterThanOrEqual(0)
  })

  test('overrides to one pass', { repeats: 0 }, ({ task }) => {
    console.log('attempt', task.name, task.result?.repeatCount)
    expect(task.result?.repeatCount).toBeGreaterThanOrEqual(0)
  })

  describe('nested suite', () => {
    test('restores the project default', { repeats: null }, ({ task }) => {
      console.log('attempt', task.name, task.result?.repeatCount)
      expect(task.result?.repeatCount).toBeGreaterThanOrEqual(0)
    })
  })
})

test('explicit one pass', { repeats: 0 }, ({ task }) => {
  console.log('attempt', task.name, task.result?.repeatCount)
  expect(task.result?.repeatCount).toBeGreaterThanOrEqual(0)
})
