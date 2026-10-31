// A constructor's parameter property is a parameter: the `var` is the same binding.
import { beforeEach, expect, test } from 'vitest'
import { cases } from './support'

class Rewritten {
  constructor(private hook: any) {
    hook(() => {})
    var hook: any = beforeEach
  }
}

new Rewritten(cases)
test('stays comparable', () => {
  expect(1).toBe(1)
})
