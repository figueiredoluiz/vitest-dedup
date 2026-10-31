import { beforeEach } from 'vitest'
import { cases } from './support'
/** Registers a hook. */
export function closed() {
  beforeEach(() => {})
}

/** Registers a hook until something rewrites it. */
var open = function () {
  beforeEach(() => {})
}
// @ts-ignore
var { run: open } = { run: cases }
export { open }
