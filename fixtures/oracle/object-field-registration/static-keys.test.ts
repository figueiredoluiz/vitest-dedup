// Computed names define own fields, including __proto__.
import { expect, test } from 'vitest'
;({ ['__proto__']: test.extend({}) }).__proto__('own prototype key', () => expect(1).toBe(1))
;({ ['run' as const]: test.extend({}) }).run('asserted key', () => expect(1).toBe(1))
