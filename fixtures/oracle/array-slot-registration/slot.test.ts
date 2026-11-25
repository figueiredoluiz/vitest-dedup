import { expect, test } from 'vitest'

;[test.extend({})][0]('first slot', () => expect(2).toBe(2))
;[, (test['extend']({}) as any)]['1']('after a hole', () => expect(5).toBe(5))
