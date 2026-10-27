// RECALL positive -- duplicate-test expected. A supported in-source binding registers nothing itself; the two declared tests remain comparable.
const register = () => {
  const { test, expect } = import.meta.vitest
  test('first', () => { expect(1).toBe(1) })
  test('second', () => { expect(1).toBe(1) })
}
register()
