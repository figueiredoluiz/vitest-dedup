// RECALL positive -- duplicate-each-row, shape: the suite import names a project file that
// re-exports the API. The prefix is read from the inventory's provenance (the re-export resolved
// to vitest), not from the import's spelling, so the rows are compared exactly as when the file
// imports from 'vitest' directly.
import { describe, test, expect } from './u'
describe('s', () => {
  test.each([1, 1])('row %i', (n) => {
    expect(n).toBe(1)
  })
})
