// `globals` is off for this project, so `describe` is not defined and the guarded block never
// registers anything; read under `globals: true`, it would hold a title collision.
if (typeof describe === 'function') {
  describe('legacy', () => {
    it('keeps the old behavior', () => {
      expect(1).toBe(1)
    })

    it('keeps the old behavior', () => {
      expect(2).toBe(2)
    })
  })
}
