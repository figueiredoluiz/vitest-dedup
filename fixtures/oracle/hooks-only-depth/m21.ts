import { beforeEach } from 'vitest'
import { f22 } from './m22'

/** One link of the import chain. */
export function f21() {
  beforeEach(() => {})
  f22()
}
