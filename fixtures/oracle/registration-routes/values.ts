import { test as base } from 'vitest'

const enabled = true
const noop = (..._args: unknown[]) => {}
const localAlias = base
export const exportedAlias = base

export const plain = enabled ? localAlias : noop
export const exported = enabled ? exportedAlias : noop
export const nested = enabled && (enabled ? exportedAlias : noop)
export const closed = false ? exportedAlias : noop
export const opaque = Boolean(process.platform) ? exportedAlias : noop
