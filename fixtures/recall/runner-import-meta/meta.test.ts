// RECALL positive -- duplicate-test, shape: a runner whose registered callback reads
// `import.meta`. The value is fixed per module, so reading it cannot make two invocations
// differ: the invocation is expanded and compared with the hand-written twin.
import { expect, test } from 'vitest'
import { run } from './runner'

run('page')

test('page reads module metadata', () => {
  expect(import.meta.url).toBeTruthy()
})
