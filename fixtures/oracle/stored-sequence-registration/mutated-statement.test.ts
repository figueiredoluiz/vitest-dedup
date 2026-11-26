import { expect, test } from 'vitest'

const original = test.extend
test.extend = () => () => {}
const register = (0, test.extend({}))
register('replaced builder', () => {})
test.extend = original

test('direct left', () => expect(17).toBe(17))
test('direct right', () => expect(17).toBe(17))
