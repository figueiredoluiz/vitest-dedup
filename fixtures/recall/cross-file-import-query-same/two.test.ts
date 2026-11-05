import { it, expect } from 'vitest'
import * as m from './recipes?raw'
it('reads', () => { expect(typeof m.default).toBe('string') })
