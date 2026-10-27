// RECALL positive -- duplicate-test expected, shape: imported data handed to a call the analyzer
// does not read.
//
// A `Symbol` key and an array of plain objects cannot be called, so the file stays comparable even
// though `data.ts` has load-time effects, and `first` pairs with `second`.
import { test, expect } from 'vitest'
import { KEY, ROWS } from './data'

const registry = new Map()
registry.set(KEY, ROWS)

test('first', () => {
  expect(1).toBe(1)
})

test('second', () => {
  expect(1).toBe(1)
})
