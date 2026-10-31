import { beforeEach } from 'vitest'
import { f6 } from './m6'

/** One link of the import chain. */
export function f5() {
  beforeEach(() => {})
  f6()
}
