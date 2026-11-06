import { beforeEach } from 'vitest'
import { f20 } from './m20'

/** One link of the import chain. */
export function f19() {
  beforeEach(() => {})
  f20()
}
