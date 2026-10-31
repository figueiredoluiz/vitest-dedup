import { beforeEach } from 'vitest'
import { setup } from './a'

/** Registers a hook, then calls the head of the cycle. */
export function around() {
  beforeEach(() => {})
  setup()
}
