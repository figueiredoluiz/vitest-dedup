/** Registers two tests through the in-source API. */
export function run() {
  const { test, expect } = import.meta.vitest
  test('first', () => {
    expect(1).toBe(1)
  })
  test('second', () => {
    expect(1).toBe(1)
  })
}
