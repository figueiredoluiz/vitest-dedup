import { beforeEach } from 'vitest'
import { f9 } from './m9'

/** One link of the import chain. */
export function f8() {
  beforeEach(() => {})
  f9()
}
