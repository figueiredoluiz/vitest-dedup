import { beforeEach } from 'vitest'
import { cases } from './support'
/** Registers a hook. */
export function closed() {
  beforeEach(() => {})
}

/** Registers a hook until something rewrites it. */
export let open = function () {
  beforeEach(() => {})
}

/** Rewrites the live binding importers read. */
export function swap() {
  open = cases
}
