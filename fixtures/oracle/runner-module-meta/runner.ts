import { expect, test } from 'vitest'

/** Registers an assertion that observes the module where its callback was written. */
export function run(label: string) {
  test(label, () => {
    expect(import.meta.url.endsWith('/runner.ts')).toBe(true)
  })
}
