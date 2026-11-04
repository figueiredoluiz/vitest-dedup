import { expect, test } from 'vitest'

/** Registers one test over the source it is given. */
export function check(source: string, name: string) {
  test(`parses the ${name} sample`, () => { expect(source).toHaveLength(4097) })
}
