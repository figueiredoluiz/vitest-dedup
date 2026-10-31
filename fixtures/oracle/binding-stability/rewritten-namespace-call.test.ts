// A rewritten namespace alias, called where it is rewritten; `kept` is the same alias untouched.
import * as vitest from 'vitest'
import { cases } from './support'

const kept: any = vitest
let api: any = vitest
api = { beforeEach: cases }

kept.beforeEach(() => {})
api.beforeEach(() => {})
const on = true
;(on ? api.beforeEach : api.beforeEach)(() => {})
vitest.test('stays comparable', () => {
  vitest.expect(1).toBe(1)
})
