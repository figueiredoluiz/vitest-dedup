/** Calls a hoisted function that registers through names declared after the call. */
export function run() {
  inner()
  const { test, expect } = import.meta.vitest
  function inner() {
    test('first', () => {
      expect(1).toBe(1)
    })
    test('second', () => {
      expect(1).toBe(1)
    })
  }
}
