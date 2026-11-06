import { beforeEach } from 'vitest'
import { f24 } from './m24'

/** One link of the import chain. */
export function f23() {
  beforeEach(() => {})
  f24()
}
