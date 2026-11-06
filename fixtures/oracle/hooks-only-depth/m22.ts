import { beforeEach } from 'vitest'
import { f23 } from './m23'

/** One link of the import chain. */
export function f22() {
  beforeEach(() => {})
  f23()
}
