import { beforeEach } from 'vitest'
import { f13 } from './m13'

/** One link of the import chain. */
export function f12() {
  beforeEach(() => {})
  f13()
}
