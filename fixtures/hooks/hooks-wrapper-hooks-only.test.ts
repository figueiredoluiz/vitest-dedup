// Calling a helper that registers only hooks registers no test: Vitest collects the two tests below
// and nothing for the `setupServer()` call, whose hooks run for the tests of its describe only.
// One edit away (a `test(...)` inside `setupServer`) the call would register a test of its own.
import { describe, expect, test } from 'vitest'
import { server, setupServer } from './server-hooks'

describe('with the server', () => {
  setupServer()
  test('the server is listening', () => { expect(server.listening).toBe(1) })
})

describe('without the server', () => {
  test('nothing is listening', () => { expect(server.listening).toBe(0) })
})
