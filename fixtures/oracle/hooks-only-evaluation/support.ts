import { test } from 'vitest'

/** Registers a test. */
export function cases() {
  test('registered by the helper', () => {})
}

/** A value whose `then` registers a test when a promise resolves with it. */
export const thenable = {
  get then() {
    cases()
    return undefined
  },
}

/** A timeout whose comparison registers a test. */
export const coerced = {
  valueOf() {
    cases()
    return 1000
  },
}

Object.defineProperty(globalThis, 'hiddenAccessor', {
  configurable: true,
  get() {
    cases()
    return 1000
  },
})
