import { beforeEach } from 'vitest'

/** Registers a hook through a local, beside an unrelated parameter. */
export function closed(other: unknown) {
  var hook = beforeEach
  hook(() => {})
}

/** Calls its parameter: the `var` only replaces it afterwards. */
export function open(hook: any) {
  hook(() => {})
  var hook: any = beforeEach
}

/** The same, as an arrow function. */
export const openArrow = (hook: any) => {
  hook(() => {})
  var hook: any = beforeEach
}
