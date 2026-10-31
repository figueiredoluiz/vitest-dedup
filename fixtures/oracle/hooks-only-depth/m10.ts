import { beforeEach } from 'vitest'
import { f11 } from './m11'

/** One link of the import chain. */
export function f10() {
  beforeEach(() => {})
  f11()
}
