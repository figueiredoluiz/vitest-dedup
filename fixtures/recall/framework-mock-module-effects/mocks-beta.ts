// A mock module imported by tests. Importing it runs `vi.hoisted` and `vi.mock` at module level;
// `useMocks` is the call a test file makes to opt in.
import { vi } from 'vitest'

vi.hoisted(() => ({ count: 0 }))

vi.mock('./api', () => ({
  fetchUser: () => ({ id: 2, name: 'beta' }),
}))

/** Setup helper `useMocks` of this case, called from its test file(s). */
export function useMocks() {}
