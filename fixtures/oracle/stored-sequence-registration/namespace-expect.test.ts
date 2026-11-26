import * as v from 'vitest'

const run = (0, v.test.extend({}))
run('namespace assertion', () => v.expect(1).toBe(1))
