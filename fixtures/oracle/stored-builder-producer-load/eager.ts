import { expect, test } from 'vitest'

export const run = (0, test.extend({}))

run('producer setup', () => expect(1).toBe(1))
