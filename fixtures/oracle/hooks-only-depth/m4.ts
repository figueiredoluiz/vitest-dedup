import { beforeEach } from 'vitest'
import { f5 } from './m5'

/** One link of the import chain. */
export function f4() {
  beforeEach(() => {})
  f5()
}
