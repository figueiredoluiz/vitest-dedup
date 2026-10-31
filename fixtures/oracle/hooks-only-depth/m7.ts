import { beforeEach } from 'vitest'
import { f8 } from './m8'

/** One link of the import chain. */
export function f7() {
  beforeEach(() => {})
  f8()
}
