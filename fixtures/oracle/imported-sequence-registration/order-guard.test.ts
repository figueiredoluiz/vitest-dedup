import { expect, test } from 'vitest'
import './setup'
import { forwarded } from './barrel'

forwarded('suppressed by setup', () => expect(2).toBe(2))
test('setup control left', () => expect(3).toBe(3))
test('setup control right', () => expect(3).toBe(3))
