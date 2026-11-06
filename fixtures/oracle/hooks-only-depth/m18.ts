import { beforeEach } from 'vitest'
import { f19 } from './m19'

/** One link of the import chain. */
export function f18() {
  beforeEach(() => {})
  f19()
}
