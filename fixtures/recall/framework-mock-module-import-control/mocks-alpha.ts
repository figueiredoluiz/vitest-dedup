// A mock module imported for its effects only: importing it runs `vi.hoisted` and `vi.mock`.
import { vi } from 'vitest'

vi.hoisted(() => ({ count: 0 }))

vi.mock('./api', () => ({
  fetchUser: () => ({ id: 1, name: 'alpha' }),
}))
