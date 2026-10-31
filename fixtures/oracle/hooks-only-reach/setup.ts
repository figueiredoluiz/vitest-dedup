import { afterEach, beforeEach, test } from 'vitest'
import { cases } from './cases'

/** Registers a hook. */
export function setup() {
  beforeEach(() => {})
}
setup.test = test

// Hooks only when called; what it returns may register anything.
/** Registers a hook and hands back the registration function. */
export function factory() {
  beforeEach(() => {})
  return test
}

// One call away from `pairOne`: it reads a registration function as a value.
/** Registers through a registration function read as a value. */
function readsTest() {
  ;[test].forEach((register) => register('read as a value', () => {}))
}

/** Registers a hook, then calls what reads `test` as a value. */
export function escaping() {
  beforeEach(() => {})
  readsTest()
}

/** Registers a hook, and may call its pair. */
export function pairOne(again: boolean) {
  afterEach(() => {})
  if (again) pairTwo(false)
}

/** Registers a hook, and may call its pair. */
export function pairTwo(again: boolean) {
  beforeEach(() => {})
  if (again) pairOne(false)
}
