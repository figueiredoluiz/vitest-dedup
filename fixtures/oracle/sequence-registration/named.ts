import { test } from 'vitest'
function run(title, callback) { test(title, callback) }
export const held = (0, run)
