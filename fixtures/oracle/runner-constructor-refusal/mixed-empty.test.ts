import { test } from 'vitest'
import { register } from './runner'

new register('constructor with empty', 'function')
test('empty', () => {})
