// RECALL negative -- duplicate-test, shape: a runner call beside a direct test whose title calls a
// function. Collection runs that call, so the runner's scope stays withheld. Control for
// duplicate-test-runner-beside-long-title.
import { expect, test } from 'vitest'
import { check } from './runner'

/** Builds a title, so the title runs code at collection. */
function make() {
  return 'made'
}
check(1, 'a')
test('spells the check', () => { expect(1).toBe(1) })
test(make(), () => { expect(2).toBe(2) })
