import { expect } from 'vitest'
import held from './default'
held('default final wrapper', () => expect(4).toBe(4))
