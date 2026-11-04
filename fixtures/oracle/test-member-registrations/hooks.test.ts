// Every hook a test function carries as a member registers as the top-level hook does, whatever
// is chained before it.
import { it, test, expect } from 'vitest'

let n = 0
it.beforeAll(() => { n = 1 })
it.afterAll(() => { n = 0 })
test.skip.beforeEach(() => { n = 1 })
test.only.afterEach(() => { n = 1 })
test.aroundEach(async (run) => { await run() })
test.aroundAll(async (run) => { await run() })

it('reads n', () => { expect(n).toBe(1) })
