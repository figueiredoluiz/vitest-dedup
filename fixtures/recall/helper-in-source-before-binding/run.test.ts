// RECALL negative -- no duplicate-test, shape: a helper registers through the in-source binding
// before declaring it. `test('zero', ...)` reads `test` in its temporal dead zone, so Vitest fails
// the file; reading the call as the framework's would pair the three tests.
const register = () => {
  test('zero', () => { expect(1).toBe(1) })
  const { test, expect } = import.meta.vitest
  test('first', () => { expect(1).toBe(1) })
  test('second', () => { expect(1).toBe(1) })
}
register()
