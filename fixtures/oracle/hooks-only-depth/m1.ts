import { beforeEach } from 'vitest'
import { f2 } from './m2'

/** One link of the import chain. */
export function f1() {
  beforeEach(() => {})
  f2()
}
