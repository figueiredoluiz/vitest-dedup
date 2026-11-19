import { test } from 'vitest'
import * as v from 'vitest'
const noop = (..._args: unknown[]) => {}
/** Each reads `test` where it cannot be the value, so each is `noop`; `space` is the namespace;
 * `label` is a string and `count` a number. */
export const condition = test ? noop : noop
export const left = test && noop
export const sequence = (test, noop)
export const space = process.env.X ? v : v
export const label = process.env.X ? test.name : 'fallback'
export const count = [test].length
