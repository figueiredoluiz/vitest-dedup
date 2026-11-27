// A pure sequence reads the same imported test as a plain call.
import { expect, test as api } from 'vitest'
const invoke = api
;(0, api)('selected', () => expect(1).toBe(1))
;('unused', (null, invoke))('different body', () => expect('plain').toContain('la'))
api('retained left', () => expect(1).toBe(1))
api('retained right', () => expect(1).toBe(1))
;(0,
  <typeof api>api
) /* comment with (
 */
('asserted', () => expect(2).toBeGreaterThan(1))
;((0, api)<any>)('instantiated', () => expect(2).toBeGreaterThan(1))
