import { expect, test } from 'vitest'
import { consumed } from './values'
consumed('ignored left', () => expect(9).toBe(9))
consumed('ignored right', () => expect(9).toBe(9))
test('retained left', () => expect(3).toBe(3))
test('retained right', () => expect(3).toBe(3))
