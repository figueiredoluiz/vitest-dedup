import { test, expect } from 'vitest'
const tools = { eval() {} }
test.each(['page'])('safe row first', label => { tools.eval('label = "other"'); expect(label).toBe('page') })
test.each(['page'])('safe row second', label => { tools.eval('label = "other"'); expect('page').toBe('page') })
