import { beforeEach, test } from 'vitest'
import { b } from './b'

/** Registers a hook, then calls a member of the cycle. */
export function setup() {
  beforeEach(() => {})
  b()
}

/** Registers a test. */
export function cases() {
  test('registered through the cycle', () => {})
}

/** Calls into the cycle. */
export function only() {
  b()
}
