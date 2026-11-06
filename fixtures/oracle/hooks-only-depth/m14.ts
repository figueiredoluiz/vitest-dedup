import { beforeEach } from 'vitest'
import { f15 } from './m15'

/** One link of the import chain. */
export function f14() {
  beforeEach(() => {})
  f15()
}
