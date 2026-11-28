import { expect } from 'vitest'
import { run } from './builder'
run('helper setup case', () => expect(1).toBe(1))
export { run }
