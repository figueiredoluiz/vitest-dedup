import { test } from 'vitest'
const enabled = true
const noop = (..._args: unknown[]) => {}
export default (enabled ? test : noop)
