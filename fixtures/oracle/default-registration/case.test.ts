import { expect, test } from 'vitest'
import chosen from './values'
chosen('default left', () => expect(1).toBe(1))
chosen('default right', () => expect(1).toBe(1))
test('direct left', () => expect(2).toBe(2))
test('direct right', () => expect(2).toBe(2))
