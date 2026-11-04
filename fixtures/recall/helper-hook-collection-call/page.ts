import { beforeAll, test } from 'vitest'

/** Registers a test when called. */
function register() {
  test('extra', () => {})
  return 1
}

/** Registers a hook and returns a function that registers a test when called. */
export function setupPage() {
  beforeAll(() => {})
  return { value: register }
}
