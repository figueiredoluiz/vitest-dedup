import { expect, test } from 'vitest'
import { extended } from './factories'

const it = extended()
it('a', ({ n }) => { expect(n).toBe(1) })
it('b', ({ n }) => { expect(n).toBe(1) })
test('c', () => { expect(3).toBe(3) })
