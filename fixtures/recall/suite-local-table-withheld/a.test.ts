// RECALL negative -- duplicate-each-row, shape: the suite is a table bound to a local name. Vitest
// registers `scope a > row` and `scope b > row` twice each; the site records no table through the
// binding, so its prefix stays unknown and no row claim names the raw `scope %s` pattern. Control
// for suite-api-reexport-spelling.
import { describe, test, expect } from 'vitest'
const suite = describe.each(['a', 'b'])
suite('scope %s', () => {
  test.each([[1], [1]])('row', (value) => {
    expect(value).toBe(1)
  })
})
