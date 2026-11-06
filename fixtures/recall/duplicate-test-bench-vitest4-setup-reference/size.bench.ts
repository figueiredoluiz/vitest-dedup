// RECALL negative -- a `setup` option passed by reference, on Vitest 4.
//
// `{ setup }` names a function declared elsewhere: it still runs around each benchmark and grows
// `size`, so the two equal bodies measure different inputs. Only options that are plain data are
// admitted, so neither is compared.
//
// Load-bearing: replace both `{ setup }` with `{ iterations: 10 }` and the pair becomes one
// `duplicate-test`.
import { bench } from 'vitest'

let size = 0

/** Grows the buffer each benchmark fills. */
function setup() {
  size += 1
}

bench('fills a buffer', () => {
  new Array(size).fill(0)
}, { setup })

bench('fills a buffer again', () => {
  new Array(size).fill(0)
}, { setup })
