import { expect } from 'vitest'
import { run } from './builder'

/** Registers a test only when the consumer calls this function. */
export function later() {
  run('deferred helper case', () => expect(4).toBe(4))
}

export { run }
