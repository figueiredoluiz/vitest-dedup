// RECALL positive -- duplicate-test, shape: rows of two tables with equal inputs, titled by long
// `$name` values, each row also holding a value the analyzer cannot read (`unit()`). Vitest 5.0.5
// renders `name` regardless and displays a `$` value past 40 UTF-16 units as its first 39 and
// `…`, so the pair is reported under those truncated titles, not the raw `$name` pattern.
import { describe, expect, test } from 'vitest'

/** The unit every row carries. */
function unit() {
  return 'kg'
}

describe('totals', () => {
  test.each([{ name: 'doubles the value given in the first table of cases', value: 2, unit: unit() }])(
    '$name',
    ({ value }) => {
      expect(value * 2).toBe(4)
    },
  )

  test.each([{ name: 'doubles the value given in the second table of cases', value: 2, unit: unit() }])(
    '$name',
    ({ value }) => {
      expect(value * 2).toBe(4)
    },
  )
})
