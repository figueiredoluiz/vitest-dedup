import { it, expect } from 'vitest'
import { servings } from './recipes'
it('reads', () => { expect(servings).toBe(4) })
