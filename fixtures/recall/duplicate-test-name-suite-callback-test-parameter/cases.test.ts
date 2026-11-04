// RECALL variant -- duplicate-test-name, shape: tests registered through the `test` a suite
// callback receives. Vitest calls a suite factory with its `test` API, so `s > formats` is
// registered twice; the analyzer cannot attribute those calls, so they are refused visibly
// (`registered-elsewhere`, incomplete) rather than dropped. The plain pair outside the suite is
// an ordinary name collision and must still be found.
//
// Named weakening: drop the refusal and the two parameter tests vanish from the count while the
// run reads as complete.
import { describe, expect, test as plain } from 'vitest'

describe('s', (test) => {
  test('formats', () => {
    expect(String(1)).toBe('1')
  })

  test('formats', () => {
    expect(String(2)).toBe('2')
  })
})

plain('parses the header', () => {
  expect(Number('1')).toBe(1)
})

plain('parses the header', () => {
  const parsed = Number.parseInt('2', 10)
  expect(parsed).toBe(2)
})
