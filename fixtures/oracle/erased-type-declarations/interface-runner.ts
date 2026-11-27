import { expect, test } from 'vitest'

export interface register {
  (title: string): void
}

/** Register an ordinary case after the interface has erased. */
export function register(title: string) {
  test(title, () => expect(1).toBe(1))
}
