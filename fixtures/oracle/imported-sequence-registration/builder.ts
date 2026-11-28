import { test } from 'vitest'

export const run = (0, test.extend({}))
export const inert = (test.extend({}), (_title: string, _body: () => void) => {})
