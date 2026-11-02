// RECALL config case -- `duplicate-test-name` in projects whose config forwards a shared one.
//
// `packages/api` re-exports the shared config's default and `packages/web` imports it and exports
// it again: each project's config IS `vitest.shared.ts`, so its `globals: true` makes the bare
// declarators Vitest calls. `packages/legacy` forwards `vitest.node.ts`, which leaves `globals`
// off, so its bare `describe` is not a Vitest call.
//
// Named weakening: read every forward as `globals: true` and `packages/legacy`'s title collision
// is reported; refuse the forward and the case reports 0 tests.
import { defineConfig } from 'vitest/config'

export default defineConfig({ test: { projects: ['packages/api', 'packages/web', 'packages/legacy'] } })
