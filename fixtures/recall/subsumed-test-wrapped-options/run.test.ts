import { describe, expect, test } from 'vitest'

const data = { total: 2, count: 3, sku: 'a', size: 4, zone: 5, tax: 6, name: 'b', city: 'c' }

test('reads total', () => { expect(data.total).toBe(2) })
test('skipped larger', { skip: true } as const, () => {
  expect(data.total).toBe(2)
  expect(data.count).toBe(3)
})

test('reads sku', () => { expect(data.sku).toBe('a') })
describe('pending suite', { todo: true } satisfies { todo: boolean }, () => {
  test('pending larger', () => {
    expect(data.sku).toBe('a')
    expect(data.size).toBe(4)
  })
})

const skipped = true
test('reads zone', () => { expect(data.zone).toBe(5) })
test('dynamic larger', { skip: skipped } as const, () => {
  expect(data.zone).toBe(5)
  expect(data.tax).toBe(6)
})

test('reads name', () => { expect(data.name).toBe('b') })
test('running larger', () => {
  expect(data.name).toBe('b')
  expect(data.city).toBe('c')
})
