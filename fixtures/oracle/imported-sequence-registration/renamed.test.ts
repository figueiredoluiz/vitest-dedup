import { expect } from 'vitest'
import { forwarded as selected } from './barrel'

selected('re-exported selection', () => expect(9).toBe(9))
