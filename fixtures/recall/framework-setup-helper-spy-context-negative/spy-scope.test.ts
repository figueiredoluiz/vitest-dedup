// RECALL negative -- duplicate-test, control for framework-setup-helper-shapes.
//
// The bodies are identical, but only the first suite runs with the hooks `spyOnInfo()` registers
// (a spy installed before each test and restored after it); the second runs with none. Different
// hook contexts make them different tests, so no finding is wanted before or after helpers are
// analyzed per effect. It fails a fix that marks every helper inert (the hooks would vanish from
// the context and the pair would be reported). Call `spyOnInfo()` in both suites, or in neither,
// and the pair becomes a duplicate-test.
import { describe, expect, test } from 'vitest'
import { spyOnInfo } from './helper-spy'

describe('with the spy installed', () => {
  spyOnInfo()

  test('logs once', () => {
    console.info('ready')
    expect(console.info).toBeTypeOf('function')
  })
})

describe('without the spy', () => {
  test('logs once, plain', () => {
    console.info('ready')
    expect(console.info).toBeTypeOf('function')
  })
})
