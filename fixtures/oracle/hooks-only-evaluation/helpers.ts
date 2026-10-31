import { beforeEach } from 'vitest'
import * as support from './support'
import { cases, thenable } from './support'

/** Registers a hook. */
export function hooks() {
  beforeEach(() => {})
}

/** Registers a hook, then calls through a namespace. */
export function namespaceOpen() {
  beforeEach(() => {})
  support.cases()
}

const local = {
  /** Registers a test. */
  run() {
    cases()
  },
}

/** Registers a hook, then calls a method. */
export function methodOpen() {
  beforeEach(() => {})
  local.run()
}

/** Registers a hook and ignores what it is given. */
export function parameterClosed(run: () => void) {
  beforeEach(() => {})
}

/** Registers a hook, then calls what it is given. */
export function parameterOpen(run: () => void) {
  beforeEach(() => {})
  run()
}

/** Registers a hook, with a callee only known after a call. */
export function conditionalOpen() {
  ;(support.cases() ? beforeEach : beforeEach)(() => {})
}

/** Registers a hook, async, and resolves with nothing. */
export async function asyncClosed() {
  beforeEach(() => {})
  return
}

/** Registers a hook, async, and resolves with a value whose `then` runs. */
export async function asyncOpen() {
  beforeEach(() => {})
  return thenable
}

/** Registers a hook and returns what it is given. */
export function passing(value: unknown) {
  beforeEach(() => {})
  return value
}

/** Calls `passing` and returns its value. */
export const arrowClosed = (value: unknown) => passing(value)

/** The same, async: its implicit return is resolved. */
export const arrowOpen = async (value: unknown) => passing(value)

/** Registers a hook, reading an initializer that runs nothing. */
export function globalClosed() {
  const value = undefined
  beforeEach(() => {})
}

/** Registers a hook, reading a global that may be an accessor. */
export function globalOpen() {
  const value = hiddenAccessor
  beforeEach(() => {})
}

/** Registers a hook with a literal timeout. */
export function timeoutClosed() {
  beforeEach(() => {}, 1000)
}

/** Registers a hook with the timeout it is given. */
export function timeoutOpen(timeout: number) {
  beforeEach(() => {}, timeout)
}
