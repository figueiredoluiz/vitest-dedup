// A setup helper that registers hooks and no test: `setupServer` in MSW is the common shape.
import { afterEach, beforeEach } from 'vitest'

export const server = { listening: 0 }

export function setupServer() {
  beforeEach(() => { server.listening += 1 })
  afterEach(() => { server.listening -= 1 })
}
