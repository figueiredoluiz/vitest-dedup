import { expect, test } from 'vitest'

/** Register one check whose input remains part of its behavior. */
export function register(label: string, value: number) {
  test(label, () => { expect(value + 1).toBeGreaterThan(1) })
}
