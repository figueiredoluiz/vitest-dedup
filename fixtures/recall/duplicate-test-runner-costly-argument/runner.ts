import { expect, test } from 'vitest'

/** Registers one check that `value` renders as a plain object does. */
export function check(value: string, name: string) {
  test(`checks ${name}`, () => { expect(value).toBe('[object Object]') })
}
