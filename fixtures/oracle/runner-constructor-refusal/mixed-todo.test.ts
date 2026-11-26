import { test } from 'vitest'
import { register } from './runner'

new register('constructor with todo', 'function')
test.todo('todo')
