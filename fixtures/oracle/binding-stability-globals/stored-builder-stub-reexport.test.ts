import { expect, test as control } from 'vitest'
import { tools } from './mock-tools'

tools.stubGlobal('test', { extend: () => () => {} })
const run = (0, test.extend({}))
run('replaced barrel builder', () => expect(false).toBe(true))

control('barrel stub left', () => expect(12).toBe(12))
control('barrel stub right', () => expect(12).toBe(12))
