import { expect } from 'vitest'
import { run } from './builder'
export const result = run('exported initializer setup case', () => expect(2).toBe(2))
export { run }
