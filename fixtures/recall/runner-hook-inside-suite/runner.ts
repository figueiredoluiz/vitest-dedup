// A runner that registers a `beforeEach` and a test where it is called.
import { beforeEach, expect, test } from 'vitest'

let ready = 0

/** Registers the caller suite's hook and its reading test. */
export function install() {
  beforeEach(() => {
    ready = 1
  })
  test('reads', () => {
    expect(ready).toBe(1)
  })
}
