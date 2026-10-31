import { hook } from './hook-alias'

/** Registers through an imported hook with a literal timeout. */
export function closed() {
  hook(() => {}, 1000)
}

/** Registers through an imported hook with the timeout it is given. */
export function open(timeout: number) {
  hook(() => {}, timeout)
}
