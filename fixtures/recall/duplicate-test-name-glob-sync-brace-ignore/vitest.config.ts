// RECALL config case -- `duplicate-test-name` in a project listed by `glob.sync` with a brace `ignore`.
//
// glob reads each brace alternative as its own pattern, so each `/**` alternative drops the
// directory it names: `packages/*` leaves only `packages/core`. `packages/e2e` and
// `packages/unused` hold ignored projects whose files are not Vitest tests.
//
// Named weakening: drop either alternative and its directory becomes a project, so its own title
// collision must then be reported.
import { glob } from 'glob'
import { defineConfig } from 'vitest/config'

const packageDirs = glob.sync('packages/*', {
  ignore: '{packages/e2e/**,packages/unused/**}',
})

export default defineConfig({ test: { projects: packageDirs } })
