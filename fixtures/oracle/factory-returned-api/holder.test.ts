import { expect } from 'vitest'
import { holder } from './factories'

const { it } = holder()
it('a', ({ n }) => { expect(n).toBe(1) })
