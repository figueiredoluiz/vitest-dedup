import { beforeEach } from 'vitest'
import { f12 } from './m12'

/** One link of the import chain. */
export function f11() {
  beforeEach(() => {})
  f12()
}
