import { beforeEach } from 'vitest'
import { f17 } from './m17'

/** One link of the import chain. */
export function f16() {
  beforeEach(() => {})
  f17()
}
