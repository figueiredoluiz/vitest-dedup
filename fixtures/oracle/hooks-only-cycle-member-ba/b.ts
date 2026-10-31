import { test } from 'vitest'
import { a } from './a'

/** Registers a test, then calls into the cycle. */
export function b(again: boolean) {
  test('registered in the cycle', () => {})
  if (again) a(false)
}
