// RECALL positive -- duplicate-test expected, shape: a helper whose registering default never runs
// because the call passes an argument.
//
// One edit from `local-helper-default-hook`: `declare(null)` supplies the parameter, and a default
// runs only for `undefined`, so `beforeEach` is never registered. The body is read and `hidden` is
// compared with `visible`.
import { test, expect, beforeEach } from 'vitest'

/** Declares `hidden`; its default would register a hook if it ever ran. */
function declare(_hook: unknown = beforeEach(() => {})) {
  test('hidden', () => {
    expect(1).toBe(1)
  })
}

declare(null)

test('visible', () => {
  expect(1).toBe(1)
})
