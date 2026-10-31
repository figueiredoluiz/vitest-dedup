import { beforeEach } from 'vitest'
import { cases } from './support'

const kept = beforeEach
let moved = beforeEach
moved = cases
const alias = moved

/** Registers through an alias never written. */
export function closed() {
  kept(() => {})
}

/** Registers through a copy of a rewritten alias. */
export function open() {
  alias(() => {})
}
