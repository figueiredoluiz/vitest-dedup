// Control one edit away: with no call before the member, the suite is the top-level one and
// these copies are cross-file duplicates.
import { expect, test } from 'vitest'

test.skip.describe('formats the label', () => {
  test('trims it', () => { expect(' a '.trim()).toBe('a') })
})
