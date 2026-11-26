import { expect, test } from 'vitest'
import * as vitest from 'vitest'

const register = (test('prefix', () => expect(3).toBe(3)), vitest.test.extend({}))
register('after prefix', () => expect(4).toBe(4))
