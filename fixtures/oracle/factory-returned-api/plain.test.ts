import { expect } from 'vitest'
import { plain } from './factories'

const it = plain()
it('a', () => { expect(1).toBe(1) })
