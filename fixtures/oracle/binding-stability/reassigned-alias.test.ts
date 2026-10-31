// An alias of a helper, reassigned, may run anything where it is called.
import { beforeEach, describe, expect, test } from 'vitest'
import { cases } from './support'

/** Registers a hook. */
function hooks() {
  beforeEach(() => {})
}

const kept = hooks
let moved = hooks
// @ts-ignore
moved = cases

describe('reassigned-alias', () => {
  kept()
  moved()
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
