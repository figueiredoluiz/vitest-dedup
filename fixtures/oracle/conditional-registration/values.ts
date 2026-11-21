import { test as base } from 'vitest'
const enabled = true
const noop = (..._args: unknown[]) => {}
export const chosen = enabled ? base : noop
export const absent = enabled ? noop : noop
export const scalar = !enabled ? base.name : noop
const label = base.name
export const aliasedScalar = !enabled ? label : noop
export { chosen as default }
