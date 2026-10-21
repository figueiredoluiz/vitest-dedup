// RECALL control -- duplicate-test, parent ignore file with an unanchored directory name.
//
// The case directory is ONE repository (this checkout), and its `.gitignore` names `api`, so git
// itself ignores `project/api/`: the tests there are not analyzed and that is correct. Only
// `project/web/` is. This is the control for a nested repository: when `project/` is its own
// repository (a layout fixtures cannot hold, because git refuses to track a nested `.git`), the
// parent's rule must stop at its boundary and both directories are analyzed; see
// fixtures/recall/README.md. Add a `.git` directory to `project/` and the `api` pair is analyzed too.
import { expect, test } from 'vitest'

test('pages: first check', () => {
  expect(1 + 1).toBe(2)
})

test('pages: second check', () => {
  expect(1 + 1).toBe(2)
})
