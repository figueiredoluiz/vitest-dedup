// A server-setup helper: owns a module-level server object and registers lifecycle hooks.
import { afterAll, beforeAll } from 'vitest'

const server = {
  listen() {},
  close() {},
}

/** Setup helper `setupServer` of this case, called from its test file(s). */
export function setupServer() {
  beforeAll(() => server.listen())
  afterAll(() => server.close())
}
