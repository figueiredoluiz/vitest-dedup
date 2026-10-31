import { cases } from './support'

globalThis.beforeEach = cases

/** Registers through a global the module rewrote. */
export function open() {
  beforeEach(() => {})
}
