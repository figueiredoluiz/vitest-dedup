import { beforeEach } from 'vitest'
import { f32 } from './m32'

/** One link of the import chain. */
export function f31() {
  beforeEach(() => {})
  f32()
}
