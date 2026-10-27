import { test } from 'vitest'

test(/** Supplies only the test title; its body must never execute. */ function alpha() { throw Error('title') })
test(/** Supplies only the test title; its body must never execute. */ function beta() { throw Error('title') }, {})
