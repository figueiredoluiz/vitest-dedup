import { test } from 'vitest'
const enabled = true
export const and = enabled && test
export const or = false || test
export const coalesced = null ?? test
export const consumed = test && ((..._args: unknown[]) => {})
