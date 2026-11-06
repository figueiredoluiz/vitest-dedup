import { beforeEach } from 'vitest'
import { f16 } from './m16'

/** One link of the import chain. */
export function f15() {
  beforeEach(() => {})
  f16()
}
