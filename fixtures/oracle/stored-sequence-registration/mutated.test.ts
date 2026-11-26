import { expect, test } from 'vitest'

const original = test.extend
const register = ((test.extend = () => () => {}), test.extend({}))
register('replaced builder', () => {})
test.extend = original

test('comparable left', () => expect(9).toBe(9))
test('comparable right', () => expect(9).toBe(9))
