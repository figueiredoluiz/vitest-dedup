import { expect, test } from 'vitest'
import { NUMBER } from './scalar-values'

const local = 1
test.each([[local], [1]])('local control', value => expect(value).toBe(1))
test.each([[NUMBER], [1]])('imported control', value => expect(value).toBe(1))
