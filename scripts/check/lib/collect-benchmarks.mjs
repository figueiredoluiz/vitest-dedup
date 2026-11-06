#!/usr/bin/env node
// Vitest 4's own reading of a directory's benchmark files, without running them.
//
// Vitest 5 has no top-level `bench`, so the oracle reads a directory that declares Vitest 4 with
// the pinned `vitest4` package: `createVitest('benchmark')` and `collect()` list each benchmark
// file's tasks. Vitest maps the files' `vitest` imports to itself, so the checkout's Vitest 5
// beside it does not leak in. Prints one JSON object: per file path relative to the directory, the
// `' > '`-joined titles of its tasks and its collection errors.
import { relative } from 'node:path'
import { createVitest } from 'vitest4/node'

/**
 * Every task under `task` with its suite path, as the probe runner titles tests.
 * @param {object} task A collected file or suite.
 * @param {string[]} path The enclosing suite names.
 * @returns {string[]} The titles.
 */
function titles(task, path) {
  return task.tasks.flatMap((child) =>
    child.type === 'suite' ? titles(child, [...path, child.name]) : [[...path, child.name].join(' > ')],
  )
}

const root = process.argv[2]
const vitest = await createVitest('benchmark', { root, watch: false, config: false })
try {
  const { testModules, unhandledErrors } = await vitest.collect()
  const files = {}
  for (const module of testModules) {
    files[relative(root, module.moduleId).split('\\').join('/')] = {
      titles: titles(module.task, []),
      errors: (module.task.result?.errors ?? []).map((error) => String(error.message).split('\n')[0]),
    }
  }
  process.stdout.write(JSON.stringify({ files, unhandled: unhandledErrors.length }))
  // A file that failed to load is reported above, not as this script's failure.
  process.exitCode = 0
} finally {
  await vitest.close()
}
