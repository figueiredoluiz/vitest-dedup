import { expect, test } from 'vitest'

export function make(mode: string) {
  return {
    cases: {
      nested: {
        read: (value: number) => {
          test(`${mode} ${value}`, () => expect({ mode, value }).toEqual({ mode, value }))
        },
      },
    },
  }
}
