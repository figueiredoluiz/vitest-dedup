import { beforeEach } from 'vitest'
import { f18 } from './m18'

/** One link of the import chain. */
export function f17() {
  beforeEach(() => {})
  f18()
}
