import { expect, test as check } from 'vitest'
import * as vitest from 'vitest'

;[check('independent prefix', () => expect(7).toBe(7)), vitest.test.extend({})][1](
  'selected slot',
  () => expect(8).toBe(8),
)
