import { expect, test } from 'vitest'

/** Registers one check of `value` under `name`. */
export function check(value: number, name: string) {
  test(`checks ${name}`, () => { expect(value).toBe(1) })
}
