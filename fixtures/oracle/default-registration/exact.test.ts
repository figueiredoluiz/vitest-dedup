import { expect, test } from 'vitest'
import chosen from './exact-values'

chosen('exact default left', () => expect(4).toBe(4))
chosen('exact default right', () => expect(5).toBe(5))
test('exact direct left', () => expect(6).toBe(6))
test('exact direct right', () => expect(6).toBe(6))
