import { onTestFinished, test } from 'vitest'

/** Returns an extended test API. */
export function extended() {
  return test.extend({ n: 1 })
}

/** Returns an extended test API from an expression body. */
export const arrow = () => test.extend({ n: 1 })

/** Returns the test API itself. */
export function plain() {
  return test
}

/** Returns one of two APIs derived from a local alias. */
export function selected(flag: boolean) {
  const t = test.extend({ n: 1 })
  return flag ? t : t.skip
}

/** Returns an object holding an extended test API. */
export function holder() {
  return { it: test.extend({ n: 1 }) }
}

/** Appends the test API to a string; the result is a string. */
export function joined() {
  let label = ''
  return label += test
}

/** Registers a hook and returns a plain result. */
export async function run() {
  onTestFinished(() => {})
  return { stderr: '', exitCode: 0 }
}
