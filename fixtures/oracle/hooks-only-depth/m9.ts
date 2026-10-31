import { beforeEach } from 'vitest'
import { f10 } from './m10'

/** One link of the import chain. */
export function f9() {
  beforeEach(() => {})
  f10()
}
