import { test } from 'vitest'

/** Registers a test. */
export function cases(..._args: unknown[]) {
  test('registered by the rewrite', () => {})
}

/** A timeout whose comparison registers a test. */
export const coerced = {
  valueOf() {
    cases()
    return 1000
  },
}
