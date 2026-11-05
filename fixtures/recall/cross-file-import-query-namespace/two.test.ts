import { it, expect } from 'vitest'
import * as m from './recipes'
it('reads', () => { expect(Object.keys(m).sort()).toEqual(['servings']) })
