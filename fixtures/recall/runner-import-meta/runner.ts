import { test, expect } from 'vitest'

/** Registers the metadata check the caller asks for; the callback reads module metadata. */
export function run(label: string) {
  test(`${label} reads module metadata`, () => {
    expect(import.meta.url).toBeTruthy()
  })
}
