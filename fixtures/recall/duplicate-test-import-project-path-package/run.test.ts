// RECALL negative -- no duplicate-test, shape: a project path and a package spelled alike.
//
// `./foo/index.js` is the project's file and `foo/index.js` the installed package's: `local`
// passes and `packaged` fails, so keying both by the same path text would pair them.
import { test, expect } from 'vitest'
import * as local from './foo/index.js'
import * as packaged from 'foo/index.js'

test('local', () => {
  expect(local.servings).toBe(4)
})

test('packaged', () => {
  expect(packaged.servings).toBe(4)
})
