import { expect } from 'vitest'
import { inert, run as selected } from './builder'

selected('named selection', () => expect(8).toBe(8))
inert('non-registering final value', () => expect(0).toBe(0))
