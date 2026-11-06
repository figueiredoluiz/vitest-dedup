// RECALL negative -- an imported runner that registers a hook, called inside suites.
//
// A hook registered inside a suite governs every test in that suite, which the expansion's
// file-wide lifecycle context does not express, so neither call expands and the two `reads`
// tests are not compared.
//
// Load-bearing: call `install()` once at the top level instead and its test expands.
import { describe } from 'vitest'
import { install } from './runner'

describe('a', () => {
  install()
})

describe('b', () => {
  install()
})
