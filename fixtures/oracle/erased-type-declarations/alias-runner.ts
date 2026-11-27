import { expect, test } from 'vitest'

/** Register an ordinary case whose binding also names a type. */
export function register(title: string) {
  type title = string
  test(title, () => expect(1).toBe(1))
}

export type register = (title: string) => void
