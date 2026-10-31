import { beforeEach } from 'vitest'
import { f3 } from './m3'

/** One link of the import chain. */
export function f2() {
  beforeEach(() => {})
  f3()
}
