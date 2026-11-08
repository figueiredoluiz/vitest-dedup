// RECALL control -- duplicate-test present, shape: two sibling suites each assign a shared binding
// in their own `beforeAll`, and each reads it.
//
// Another suite's hook writes `app`, but the reader's own `beforeAll` assigns it again before any
// of its tests, so each reader sees its own suite's value in any order. Pins that a write from
// another suite is not counted when a hook around the reader rewrites the binding after it; the
// twin `hook-before-all-nested-writer` moves the writer inside the shielding suite.
import { describe, test, expect, beforeAll } from 'vitest'

let app = 0

describe('first', () => {
  beforeAll(() => {
    app = 1
  })
  test('reads', () => {
    expect(app).toBe(1)
  })
})

describe('second', () => {
  beforeAll(() => {
    app = 1
  })
  test('reads', () => {
    expect(app).toBe(1)
  })
})
