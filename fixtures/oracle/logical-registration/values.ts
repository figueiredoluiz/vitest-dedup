import { test } from 'vitest'
const enabled = true
export const and = enabled && test
export const or = false || test
export const coalesced = null ?? test
export const consumed = test && ((..._args: unknown[]) => {})
export default (enabled && (enabled ? test : test))
export const scalar = false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(false || test.skipIf(0).name).name).name).name).name).name).name).name).name).name).name).name
