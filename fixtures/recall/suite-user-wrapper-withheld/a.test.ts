// RECALL negative -- duplicate-each-row, shape: the suite import names a user wrapper around the
// API. Wrapper provenance keeps the suite prefix unknown, so the rows' scope stays withheld and
// no row claim is manufactured. Control for suite-api-reexport-spelling.
import { describe, test, expect } from './u'
describe('s', () => {
  test.each([1, 1])('row %i', (n) => {
    expect(n).toBe(1)
  })
})
