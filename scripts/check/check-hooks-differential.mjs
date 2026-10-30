#!/usr/bin/env node
// Differential check of hooks-only helpers: generated helpers, collected by the pinned Vitest and
// analysed by the next engine, compared with the collection oracle's own site comparison.
//
// The corpus gates only see shapes someone wrote down, and every way around the hooks-only claim so
// far was found in review, not by a gate. `lib/hooks-differential.mjs` writes one case per known
// route around the claim and per way of declaring the helper, plus closed bodies the claim admits.
// An open case fails when Vitest collects its hidden test and the analyzer counted it nowhere; a
// closed case fails when its call is not `hook-wrapper`, because the check would then prove
// nothing. Cases the analyzer is known to get wrong are listed in `hooks-differential-known.json`
// with a reason; a listed case that starts passing fails too, so the entry goes when support lands.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { collectSites, collectWithVitest, compareSites, oracleBinary } from './lib/collection-oracle.mjs'
import { generateCases, judge } from './lib/hooks-differential.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const KNOWN = join(ROOT, 'scripts', 'check', 'hooks-differential-known.json')
// Under the checkout, so the cases resolve its `node_modules/vitest`; `target/` is ignored by git.
const WORK = 'target/hooks-differential'

/** Writes the cases, runs Vitest and the analyzer over them, and returns the exit code. */
function main() {
  const binary = oracleBinary(ROOT, process.argv[2])
  if (binary === null) return 2
  const cases = generateCases()
  const work = join(ROOT, WORK)
  rmSync(work, { recursive: true, force: true })
  try {
    for (const { id, files } of cases) {
      for (const [path, source] of Object.entries(files)) {
        mkdirSync(dirname(join(work, id, path)), { recursive: true })
        writeFileSync(join(work, id, path), source)
      }
    }
    const vitest = collectWithVitest(ROOT, WORK)
    const sites = collectSites(binary, ROOT, WORK)
    const known = JSON.parse(readFileSync(KNOWN, 'utf8')).cases
    const { failures, unsound, open, closed } = judge(cases, vitest, compareSites(vitest, sites), sites, known)
    console.log(
      `hooks differential: ${cases.length} cases (${open} open, ${closed} closed); ` +
        `${unsound.length} hidden tests the analyzer counted nowhere (${known.length} known)`,
    )
    for (const failure of failures) console.error(`FAIL: ${failure}`)
    return failures.length === 0 ? 0 : 1
  } finally {
    rmSync(work, { recursive: true, force: true })
  }
}

process.exitCode = main()
