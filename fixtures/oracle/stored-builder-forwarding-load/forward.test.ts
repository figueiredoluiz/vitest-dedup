import { expect } from 'vitest'
import { run } from './helper'
run('named caller case', () => expect(3).toBe(3))
