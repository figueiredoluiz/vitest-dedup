import { describe as d } from 'vitest'
export const describe = (name: string, fn: () => void) => d(name, fn)
export * from 'vitest'
