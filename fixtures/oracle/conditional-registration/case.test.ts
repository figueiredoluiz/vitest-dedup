import { expect, test } from 'vitest'
import chosen, { absent, aliasedScalar, scalar } from './values'
chosen('conditional left', () => expect(1).toBe(1))
chosen('conditional right', () => expect(1).toBe(1))
absent('absent', () => expect(1).toBe(1))
scalar('scalar', () => expect(1).toBe(1))
aliasedScalar('aliased scalar', () => expect(1).toBe(1))
test('direct left', () => expect(2).toBe(2))
test('direct right', () => expect(2).toBe(2))
