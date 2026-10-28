// RECALL positive -- duplicate-test expected, shape: imported values handed to a call the analyzer
// does not read.
//
// Only inline callbacks are judged, so names the call receives keep the file comparable even
// though `data.ts` has load-time effects the proof cannot clear, and `first` pairs with `second`.
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
