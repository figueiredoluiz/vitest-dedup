// Control: the hooks of framework-hook-helper-server, with inline callbacks and no module state.
import { afterAll, beforeAll } from 'vitest'
/** Setup helper `setupServer` of this case, called from its test file(s). */
export function setupServer() {
  beforeAll(() => {})
  afterAll(() => {})
}
