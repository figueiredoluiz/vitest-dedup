import { beforeEach } from 'vitest'
import { f14 } from './m14'

/** One link of the import chain. */
export function f13() {
  beforeEach(() => {})
  f14()
}
