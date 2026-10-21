// Installs a spy for each test of the calling suite, through hooks the helper registers.
import { afterEach, beforeEach, vi } from 'vitest'

/** Setup helper `spyOnInfo` of this case, called from its test file(s). */
export function spyOnInfo() {
  beforeEach(() => {
    vi.spyOn(console, 'info')
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })
}
