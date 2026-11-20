import { test, describe } from 'vitest'
const noop = (..._args: unknown[]) => {}
const on = process.env.ORACLE_OFF !== 'true'
/** Values that are a registration function only on one branch. */
export const ternary = on ? test : noop
export const first = on ? noop : describe
export const either = on && test
export const nullish = undefined ?? test
export const extended = on ? test.extend({}) : noop
export const nested = on ? noop : on ? test.extend({}) : noop
export const listed = [test][0]
export const { member } = { member: on ? test : noop }
/** Values no branch of which registers. */
export const neither = on ? noop : (..._args: unknown[]) => {}
export const none = on && noop
