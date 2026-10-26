// A module the runner imports: it mocks `./slug` while it loads.
import { vi } from 'vitest'

vi.mock('./slug', () => ({ slug: () => 'mocked' }))

export const mocked = true
