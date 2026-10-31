import { beforeEach } from 'vitest'
import { cases } from './support'
/** Registers a hook. */
export function closed() {
  beforeEach(() => {})
}

/** Registers a hook until something rewrites it. */
export function open() {
  beforeEach(() => {})
}
// @ts-ignore
open = cases
