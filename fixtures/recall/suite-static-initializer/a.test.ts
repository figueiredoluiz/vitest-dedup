// RECALL negative -- duplicate-suite, shape: a static field initializer writes module state at
// registration. Each copy's class definition runs `++n` when it registers, so the identical
// tests observe different values depending on which copy registered last: the claim is refused
// and the tests stay excluded as order-dependent. Instance-field initializers are different:
// they run at construction, during the test, and identical constructions behave alike.
import { describe, test, expect } from 'vitest'
let n = 0
describe('s', () => {
  class C {
    static value = ++n
  }
  test('t', () => {
    expect(C.value).toBe(1)
  })
})
describe('s', () => {
  class C {
    static value = ++n
  }
  test('t', () => {
    expect(C.value).toBe(1)
  })
})
