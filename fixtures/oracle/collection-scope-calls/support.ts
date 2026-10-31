import { test } from 'vitest'

/** Registers a test. */
export function cases(..._args: unknown[]) {
  test('registered from another module', () => {})
}

/** An object holding a function that registers. */
export const api = {
  /** Registers a test. */
  cases() {
    test('registered through a member', () => {})
  },
}

/** A constructor that registers. */
export class Cases {
  /** Registers a test. */
  constructor() {
    test('registered by a constructor', () => {})
  }

  /** Registers a test. */
  static run() {
    test('registered by a static method', () => {})
  }
}

/** A timeout whose comparison registers a test. */
export const coerced = {
  /** Registers a test. */
  valueOf() {
    test('registered by a comparison', () => {})
    return 1000
  },
}

/** A title that registers a test. */
export function title() {
  test('registered by a title', () => {})
  return 'titled'
}

export default api
