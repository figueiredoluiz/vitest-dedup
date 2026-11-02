import { test } from 'vitest'

test('imported control stays collected', () => {
  expect(1).toBe(1)
})

// Vitest evaluates the side-effect import before the forward, so globals are off and this block
// never registers. Admitting the forward would invent this title collision.
if (typeof describe === 'function') {
  describe('mutated', () => {
    it('stays absent', () => {
      expect(1).toBe(1)
    })

    it('stays absent', () => {
      expect(2).toBe(2)
    })
  })
}
