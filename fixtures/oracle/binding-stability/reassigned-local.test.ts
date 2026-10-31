// A helper of the test file, reassigned, may run anything where it is called.
import { beforeEach, describe, expect, test } from 'vitest'
import { cases } from './support'

/** Registers a hook. */
function closed() {
  beforeEach(() => {})
}

/** Registers a hook until something rewrites it. */
function open() {
  beforeEach(() => {})
}
// @ts-ignore
open = cases

describe('reassigned-local', () => {
  closed()
  open()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
