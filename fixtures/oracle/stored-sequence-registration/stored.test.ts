import { expect, test } from 'vitest'

const register = (0, test.extend({}))
register('stored builder', () => expect(2).toBe(2))
