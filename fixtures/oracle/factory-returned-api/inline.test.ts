import { expect } from 'vitest'
import { extended, selected } from './factories'

extended()('a', ({ n }) => { expect(n).toBe(1) })
selected(true)('b', ({ n }) => { expect(n).toBe(1) })
