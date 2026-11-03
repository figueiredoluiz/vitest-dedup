import { expect, test } from 'vitest'

/** Registers a callback that reads the invocation's own `arguments`: receiver-dependent. */
export function run(label: string) {
  test(`${label} sees arguments`, function () {
    expect(arguments.length).toBeGreaterThanOrEqual(0)
  })
}
