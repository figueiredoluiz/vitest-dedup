// RECALL negative -- subsumed-test over expected failures the options do not spell.
//
// A passing larger test proves a smaller one's events only when both expect to pass. Each larger
// test below is an expected failure Vitest runs inverted, through options the analyzer cannot
// read as written: a `fails` flag held in a variable, a spread of an options object, and a suite
// options variable. Explicit counts keep those bodies comparable so the
// expected-failure guard alone decides subsumption. None of them is reported.
//
// The `{ retry: 2 }` pair is the control: retries change nothing a passing test checks, so it
// stays one `subsumed-test` finding.
//
// Load-bearing: write `{ fails: true }` in place of `{ fails: failing }` and nothing changes;
// write `{ retry: 1 }` in its place and that pair is reported.
import { describe, expect, test } from 'vitest'

const order = { total: 40, tax: 4, sku: 'a1', size: 2, zone: 9, code: 7, name: 'ana', city: 'rio' }
const failing = true
const inverted = { fails: true }

test('reads the total', () => {
  expect(order.total).toBe(40)
})

test('reads the total, then a wrong tax', { fails: failing }, () => {
  expect(order.total).toBe(40)
  expect(order.tax).toBe(5)
})

test('reads the sku', { repeats: 0 }, () => {
  expect(order.sku).toBe('a1')
})

test('reads the sku, then a wrong size', { ...inverted, repeats: 0 }, () => {
  expect(order.sku).toBe('a1')
  expect(order.size).toBe(3)
})

describe('inverted suite', inverted, () => {
  test('misreads the zone', { repeats: 0 }, () => {
    expect(order.zone).toBe(1)
  })

  test('misreads the zone, then the code', { repeats: 0 }, () => {
    expect(order.zone).toBe(1)
    expect(order.code).toBe(7)
  })
})

test('reads the name', () => {
  expect(order.name).toBe('ana')
})

test('reads the name and city', { retry: 2 }, () => {
  expect(order.name).toBe('ana')
  expect(order.city).toBe('rio')
})
