import { beforeEach } from 'vitest'
import { f4 } from './m4'

/** One link of the import chain. */
export function f3() {
  beforeEach(() => {})
  f4()
}
