import { beforeEach } from 'vitest'
import { f1 } from './m1'

/** One link of the import chain. */
export function f0() {
  beforeEach(() => {})
  f1()
}
