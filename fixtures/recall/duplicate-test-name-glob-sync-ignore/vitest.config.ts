// RECALL config case -- `duplicate-test-name` in a project listed by `glob.sync` with `ignore`.
//
// The project list is computed: `glob.sync` with a literal `ignore`, through one `const`. Only the
// listed project's `globals: true` makes `packages/core`'s bare declarators Vitest calls, so the
// list has to be read. `packages/e2e` holds an ignored project whose files are not Vitest tests.
//
// Named weakening: drop `ignore` and `packages/e2e` becomes a project, so its own title collision
// must then be reported; drop `globals` from `packages/core` and the case reports 0 tests.
import { glob } from 'glob'
import { defineConfig } from 'vitest/config'

const packageConfigs = glob.sync('packages/*/vitest.config.ts', {
  ignore: 'packages/e2e/**',
})

export default defineConfig({ test: { projects: packageConfigs } })
