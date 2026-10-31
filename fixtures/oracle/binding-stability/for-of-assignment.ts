import { beforeEach } from 'vitest'
import { cases } from './support'
/** Registers a hook. */
export function closed() {
  beforeEach(() => {})
}

/** Registers a hook until something rewrites it. */
let open = function () {
  beforeEach(() => {})
}
for (open of [cases]) {
}
export { open }
