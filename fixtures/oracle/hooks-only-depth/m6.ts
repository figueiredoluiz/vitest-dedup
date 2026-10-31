import { beforeEach } from 'vitest'
import { f7 } from './m7'

/** One link of the import chain. */
export function f6() {
  beforeEach(() => {})
  f7()
}
