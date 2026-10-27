// The function supplies a title, not a handler: these are todos with colliding names.
// Title collisions remain findings even when there is no executable body to compare.
import { test } from 'vitest'

test(/** Supplies only the test title; its body must never execute. */ function parsesAnEmptyDocument() { throw Error('title must not execute') })
test(/** Supplies only the test title; its body must never execute. */ function parsesAnEmptyDocument() { throw Error('title must not execute') })
