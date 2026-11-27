import * as v from 'vitest'

const check = () => v.expect(14).toBe(14)
;({ prefix: v.test('field prefix', check), run: v.test.extend({}) }).run('after prefix', check)
