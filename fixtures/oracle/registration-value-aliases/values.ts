import { test as base } from 'vitest'

const enabled = true
const noop = (..._args: unknown[]) => {}
export const held = base
const renamed = held as typeof base
const name = base.name
const length = base.length

export const chosen = enabled ? held : noop
export default (enabled && renamed)
export const scalar = enabled ? noop : name
export const number = enabled ? noop : length
