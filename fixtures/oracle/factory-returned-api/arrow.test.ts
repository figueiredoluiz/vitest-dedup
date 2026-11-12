import { expect } from 'vitest'
import { arrow } from './factories'

const it = arrow()
it('a', ({ n }) => { expect(n).toBe(1) })
it('b', ({ n }) => { expect(n).toBe(1) })
