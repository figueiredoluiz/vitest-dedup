// A call chained before a test function's `describe` runs at collection: here it sets a global
// the tests read, differently in each file, so the suite is refused rather than compared.
import { expect, test, vi } from 'vitest'

test.skipIf(vi.stubGlobal('marker', 1)).describe('reads the marker', () => {
  test('sees one', () => { expect(globalThis.marker).toBe(1) })
})
