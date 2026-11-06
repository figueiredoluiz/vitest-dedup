import { beforeEach } from 'vitest'
import { f21 } from './m21'

/** One link of the import chain. */
export function f20() {
  beforeEach(() => {})
  f21()
}
