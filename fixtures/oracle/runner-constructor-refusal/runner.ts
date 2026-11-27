import { expect, test } from 'vitest'

/** Registers a case that observes whether the runner was called as a constructor. */
export function register(title: string, targetType: string) {
  test(title, () => {
    expect(typeof new.target).toBe(targetType)
  })
}
