import { expect, test } from 'vitest'
import { register } from './runner'

new register('constructor with controls', 'function')
test.todo('todo')
test('retained left', () => expect(1).toBe(1))
test('retained right', () => expect(1).toBe(1))
