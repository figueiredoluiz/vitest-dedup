// RECALL negative -- subsumed-test where the larger test may not run.
//
// Each larger test below is skipped or a todo through a route that is not its own literal
// option: an option read from a binding or shorthand, or an enclosing suite's chain or options.
// Vitest does not run any of them, so none covers the smaller test beside it.
//
// Controls that stay one `subsumed-test` finding each: `{ skip: false }`, a plain suite, and a
// skipped smaller test beside a running larger one (removing the smaller loses nothing).
//
// Load-bearing: write `{ skip: false }` in place of `{ skip: off }` and that pair is reported.
import { describe, expect, test } from 'vitest'

const stock = { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10, k: 11, l: 12 }
const more = { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10 }
const off = true
const skip = true

test('reads a', () => { expect(stock.a).toBe(1) })
test('reads a and b', { skip: off }, () => { expect(stock.a).toBe(1); expect(stock.b).toBe(2) })

test('reads c', () => { expect(stock.c).toBe(3) })
test('reads c and d', { todo: off }, () => { expect(stock.c).toBe(3); expect(stock.d).toBe(4) })

test('reads e', () => { expect(stock.e).toBe(5) })
test('reads e and f', { skip }, () => { expect(stock.e).toBe(5); expect(stock.f).toBe(6) })

test('reads g', () => { expect(stock.g).toBe(7) })
describe('pending', { todo: true }, () => {
  test('reads g and h', () => { expect(stock.g).toBe(7); expect(stock.h).toBe(8) })
})

test('reads i', () => { expect(stock.i).toBe(9) })
describe('held', { skip: off }, () => {
  test('reads i and j', () => { expect(stock.i).toBe(9); expect(stock.j).toBe(10) })
})

test('reads k', () => { expect(stock.k).toBe(11) })
describe.skip('skipped', () => {
  test('reads k and l', () => { expect(stock.k).toBe(11); expect(stock.l).toBe(12) })
})

test('reads more a', () => { expect(more.a).toBe(1) })
describe.todo('planned', () => {
  test('reads more a and b', () => { expect(more.a).toBe(1); expect(more.b).toBe(2) })
})

test('reads more c', () => { expect(more.c).toBe(3) })
test('reads more c and d', { skip: false }, () => { expect(more.c).toBe(3); expect(more.d).toBe(4) })

test('reads more e', () => { expect(more.e).toBe(5) })
describe('plain', () => {
  test('reads more e and f', () => { expect(more.e).toBe(5); expect(more.f).toBe(6) })
})

test('reads more g', { skip: off }, () => { expect(more.g).toBe(7) })
test('reads more g and h', () => { expect(more.g).toBe(7); expect(more.h).toBe(8) })
