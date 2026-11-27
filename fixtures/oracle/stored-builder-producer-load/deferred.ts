import { expect, test } from 'vitest'

export const run = (0, test.extend({}))

/** Exports a way to call the builder after module evaluation. */
export function later() {
  run('deferred case', () => expect(3).toBe(3))
}
