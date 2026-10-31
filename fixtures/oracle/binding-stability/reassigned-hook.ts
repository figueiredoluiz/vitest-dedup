import { beforeEach } from 'vitest'
import { cases } from './support'

const kept = beforeEach
let moved = beforeEach
moved = cases

/** Registers through a hook alias never written. */
export function closed() {
  kept(() => {})
}

/** Registers through an alias that was rewritten. */
export function open() {
  moved(() => {})
}
