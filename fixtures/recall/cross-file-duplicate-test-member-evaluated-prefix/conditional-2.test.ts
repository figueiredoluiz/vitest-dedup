// A conditional before a test function's `describe` is evaluated at collection too: its test sets
// a global the tests read, differently in each file, so the suite is refused rather than compared.
import { expect, test, vi } from 'vitest'

;(vi.stubGlobal('flag', 2) ? test : test).describe('reads the flag', () => {
  test('sees one', () => { expect(globalThis.flag).toBe(1) })
})
