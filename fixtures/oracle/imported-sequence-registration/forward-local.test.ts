import { expect } from 'vitest'
import { forwarded } from './import-forward'

forwarded('imported binding forwarded', () => expect(10).toBe(10))
