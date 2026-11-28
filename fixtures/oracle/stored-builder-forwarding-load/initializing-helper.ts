import { expect } from 'vitest'
import { run } from './builder'
const result = run('initializer setup case', () => expect(1).toBe(1))
export { run }
