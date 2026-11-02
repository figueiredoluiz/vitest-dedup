import { test, expect } from 'vitest'
test.each(['page'])('unsafe row first', label => { eval('label = "other"'); expect(label).toBe('page') })
test.each(['page'])('unsafe row second', label => { eval('label = "other"'); expect('page').toBe('page') })
test.each(['page'])('unsafe name first', label => { label = label; expect(eval('label')).toBe('page') })
test.each(['page'])('unsafe name second', other => { other = other; expect(eval('label')).toBe('page') })
