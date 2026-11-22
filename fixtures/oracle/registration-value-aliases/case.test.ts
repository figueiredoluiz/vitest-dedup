import { expect, test } from 'vitest'
import alias, { chosen, number, scalar } from './values'
chosen('conditional left', () => expect(1).toBe(1))
chosen('conditional right', () => expect(1).toBe(1))
alias('logical left', () => expect(2).toBe(2))
alias('logical right', () => expect(2).toBe(2))
scalar('phantom string', () => expect(3).toBe(3))
number('phantom number', () => expect(3).toBe(3))
test('direct left', () => expect(4).toBe(4))
test('direct right', () => expect(4).toBe(4))
