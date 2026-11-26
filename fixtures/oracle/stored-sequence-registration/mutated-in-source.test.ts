import { expect } from 'vitest'

import.meta.vitest.test.extend = () => () => {}
const run = (0, import.meta.vitest.test.extend({}))
run('replaced builder', () => expect(false).toBe(true))

import.meta.vitest.test('in-source left', () => expect(5).toBe(5))
import.meta.vitest.test('in-source right', () => expect(5).toBe(5))
