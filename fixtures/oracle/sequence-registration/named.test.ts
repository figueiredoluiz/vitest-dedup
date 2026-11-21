import { expect } from 'vitest'
import { held } from './named'
held('named final wrapper', () => expect(3).toBe(3))
