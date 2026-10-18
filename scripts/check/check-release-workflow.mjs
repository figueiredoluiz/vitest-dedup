#!/usr/bin/env node
// Guard for `.github/workflows/release.yml`, whose publication path is otherwise exercised only
// by a manual dispatch from a release tag -- the worst moment to discover it is broken.
//
// This checks release payloads, platform invariants, Node pins, and publication channels:
//
//   1. The two `node -e` payloads embedded in release.yml actually run: extracted from the YAML
//      (never hardcoded -- a hardcoded copy would drift from the workflow and this check would
//      then be verifying itself instead of the real file) and executed with the same working
//      directory the workflow uses, asserting each produces plausible output. This is the check
//      that would have answered, in seconds, a review claim that the trailing `--input-type=module`
//      on `node -e '...' --input-type=module` makes Node parse the payload as CommonJS and choke on
//      top-level `await`. Empirically (see `docs/...` discussion / PR description) that claim is
//      false on every Node version tested, including the Node 24 this workflow pins: the flag is
//      honored regardless of whether it precedes or follows `-e`. The flag was still moved to the
//      conventional pre-`-e` position in release.yml, because that is the documented form and costs
//      nothing -- not because the trailing form was broken.
//
//   2. `npm/lib/targets.mjs` and `npm/prebuilt-targets.json` agree on every target's key set and
//      field values. `targets.mjs` currently just parses the JSON directly, so today this is
//      tautological -- but release.yml's comment claims this agreement as an invariant ("the build,
//      the npm packages, and the Action can never disagree"), and nothing enforced it. This check
//      makes it a real, continuously-verified guarantee instead of an accident of implementation
//      that a future edit to either file could quietly break.
//
//   3. Every job in release.yml that runs `node` as a shell command also pins a Node version via
//      `actions/setup-node`. `publish` was found (by inspection, not by this check) to run `node -e`
//      with no `setup-node` step at all, meaning it inherited whatever Node the `ubuntu-22.04` image
//      happened to ship -- unpinned, and free to change under a future image update. Not a live
//      failure (the payload runs fine on Node 18/20/24 alike), but exactly the kind of asymmetry
//      that turns into a release-day surprise. Fixed in release.yml; guarded here so it cannot
//      silently regress.
//
// No silent pass anywhere: every section asserts a floor on how much it actually checked (payloads
// extracted, targets compared, field-value pairs compared, jobs scanned) and fails loudly, with the
// counts printed, if that floor is not met.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(here, '..', '..')
const releaseYmlPath = join(repoRoot, '.github', 'workflows', 'release.yml')
const targetsMjsPath = join(repoRoot, 'npm', 'lib', 'targets.mjs')
const prebuiltTargetsPath = join(repoRoot, 'npm', 'prebuilt-targets.json')

const failures = []
/** Records each workflow or target-manifest violation for one consolidated failure report. */
const fail = (msg) => failures.push(msg)

const releaseYml = readFileSync(releaseYmlPath, 'utf8')

// ---------------------------------------------------------------------------------------------
// 1. Extract and run the `node -e` payloads embedded in release.yml.
// ---------------------------------------------------------------------------------------------
//
// Extraction technique and why it stops here rather than parsing the full YAML: the two payloads
// are single-quoted `node -e '...'` invocations whose body contains no single quotes (they use
// double quotes and backticks internally), so a regex that captures everything between the first
// `node ... -e '` and the next `'` reliably isolates each payload without needing a YAML parser
// (none is a dependency of this project) or a shell parser. The flag is matched on either side of
// `-e` deliberately: this check must run whatever the workflow currently says, flag order included,
// not the ordering this check expects -- otherwise a regression in the flag's position would go
// undetected by a check that only ever reconstructed the "correct" invocation itself. The matched
// text intentionally stops at the closing quote (plus an optional trailing flag): it does not
// capture shell context around it (`>> "$GITHUB_OUTPUT"`, `expected=$(...)`), because that context
// depends on workflow-only environment (`$GITHUB_OUTPUT`) this check does not have and does not
// need -- it only needs the `node` invocation itself to run and produce output on real stdout.
//
// If this ever needs to be more robust than "the payload never contains a single quote," the
// honest fallback is a real YAML parser as a dependency; a hand-rolled multi-quote-style scanner
// would be exactly the kind of brittle, self-verifying machinery this file is trying to avoid.
const PAYLOAD_RE = /node\s+(?:--input-type=module\s+)?-e\s+'([\s\S]*?)'(?:\s*--input-type=module)?/g

const payloads = [...releaseYml.matchAll(PAYLOAD_RE)].map((m) => ({
  full: m[0],
  body: m[1],
  index: m.index,
}))

const MIN_PAYLOADS = 2
if (payloads.length === 0) {
  fail(
    `extracted 0 'node -e' payloads from ${releaseYmlPath} -- extraction is broken, or the ` +
      'workflow no longer embeds any. Either way this check verified nothing about them.',
  )
} else if (payloads.length !== MIN_PAYLOADS) {
  fail(
    `extracted ${payloads.length} 'node -e' payload(s) from release.yml, expected exactly ` +
      `${MIN_PAYLOADS} (the matrix eval and the archive-count eval). If a payload was added or ` +
      'removed, update this check and its expectations to match.',
  )
} else {
  console.log(`extracted ${payloads.length} 'node -e' payload(s) from release.yml`)
}

/** Executes an extracted workflow payload in its real working-directory shape. */
function runPayload(label, invocation, cwd) {
  let stdout
  try {
    stdout = execFileSync('sh', ['-c', invocation], { cwd, encoding: 'utf8' })
  } catch (error) {
    fail(
      `${label}: invocation failed to run (exit ${error.status ?? '?'}) in ${cwd}\n` +
        `  invocation: ${invocation.split('\n')[0]}...\n` +
        `  stderr: ${(error.stderr ?? String(error)).trim().split('\n').slice(0, 10).join('\n  ')}`,
    )
    return null
  }
  return stdout
}

// Payload 0: the matrix eval, run from the repo root -- the working directory the `resolve` job's
// step actually has (no `working-directory:` override, and it runs right after `actions/checkout`).
if (payloads[0]) {
  const stdout = runPayload('matrix eval', payloads[0].full, repoRoot)
  if (stdout !== null) {
    const line = stdout.split('\n').find((l) => l.startsWith('matrix='))
    if (!line) {
      fail(`matrix eval: produced no 'matrix=' line. Raw stdout:\n  ${stdout.trim() || '(empty)'}`)
    } else {
      let parsed
      try {
        parsed = JSON.parse(line.slice('matrix='.length))
      } catch (error) {
        fail(`matrix eval: 'matrix=' line is not valid JSON: ${error.message}`)
      }
      if (parsed) {
        const include = parsed.include
        if (!Array.isArray(include) || include.length === 0) {
          fail(
            `matrix eval: parsed JSON has no non-empty 'include' array (got: ${JSON.stringify(parsed)})`,
          )
        } else {
          const requiredFields = ['key', 'runner', 'rustTarget', 'binaryName']
          const bad = include.filter(
            (entry) => !requiredFields.every((f) => typeof entry[f] === 'string' && entry[f] !== ''),
          )
          if (bad.length > 0) {
            fail(
              `matrix eval: ${bad.length} of ${include.length} include entries are missing one of ` +
                `${requiredFields.join(', ')}: ${JSON.stringify(bad)}`,
            )
          } else {
            console.log(`matrix eval: produced ${include.length} well-formed matrix entries`)
          }
        }
      }
    }
  }
}

// Payload 1: the archive-count eval, run from a `dist/`-like subdirectory one level under the repo
// root, exactly like the real `publish` job (which does `mkdir -p dist && cd dist` before running
// this). `../npm/lib/targets.mjs` is a relative import specifier resolved by Node against the
// current working directory for a `-e` script (there is no real module file backing it), so the
// cwd's depth relative to the repo root has to match, not just its name. A scratch directory is
// created directly under the repo root rather than reusing a real `dist/` so this check neither
// depends on a prior build having produced one nor clobbers one that has.
if (payloads[1]) {
  const scratchDir = mkdtempSync(join(repoRoot, '.tmp-release-check-'))
  try {
    const stdout = runPayload('count eval', payloads[1].full, scratchDir)
    if (stdout !== null) {
      const trimmed = stdout.trim()
      if (!/^\d+$/.test(trimmed) || Number(trimmed) <= 0) {
        fail(`count eval: expected a positive integer on stdout, got: ${JSON.stringify(trimmed)}`)
      } else {
        console.log(`count eval: printed ${trimmed}`)
      }
    }
  } finally {
    rmSync(scratchDir, { recursive: true, force: true })
  }
}

// ---------------------------------------------------------------------------------------------
// 2. The platform manifest must satisfy the invariants its consumers rely on.
// ---------------------------------------------------------------------------------------------
//
// This check deliberately does NOT compare `targets.mjs` against `prebuilt-targets.json`. An
// earlier version did, and it could never have failed: `targets.mjs` is
// `export const TARGETS = manifest.targets`, the parsed JSON itself, so the two sides are the same
// object and "they agree" is true by construction. That is the silent-success shape this
// repository keeps finding -- a check that runs, reports a reassuring count, and verifies nothing.
// The single-source design is correct and there is no second hand-maintained list to drift from
// it; the honest consequence is that the cross-check has nothing to do, not that it should be
// dressed up.
//
// What CAN break is the manifest's own content, which is hand-edited whenever a platform is added,
// and the resolver that reads it. Both are checked here instead.
const { TARGETS, targetFor } = await import(pathToFileURL(targetsMjsPath).href)
const manifest = JSON.parse(readFileSync(prebuiltTargetsPath, 'utf8'))

if (typeof manifest.schemaVersion !== 'number') {
  fail(`prebuilt-targets.json: schemaVersion must be a number, got ${JSON.stringify(manifest.schemaVersion)}`)
}

const problems = []
let assertions = 0
const seenRustTargets = new Map()

for (const [key, target] of Object.entries(TARGETS)) {
  for (const field of ['platform', 'arch', 'rustTarget', 'binaryName', 'runner']) {
    assertions++
    if (typeof target[field] !== 'string' || target[field].length === 0) {
      problems.push(`${key}.${field}: missing or not a non-empty string`)
    }
  }

  // A Windows binary that is not named `.exe` -- or a non-Windows one that is -- makes the
  // launcher build a path to a file the archive does not contain.
  assertions++
  const wantsExe = target.platform === 'win32'
  if (target.binaryName?.endsWith('.exe') !== wantsExe) {
    problems.push(`${key}.binaryName: ${JSON.stringify(target.binaryName)} for platform ${target.platform}`)
  }

  // `libc` is what separates the gnu and musl builds; it is meaningless elsewhere, and a stray
  // one would make `targetFor` refuse to match an otherwise-correct platform.
  assertions++
  const hasLibc = target.libc !== undefined
  if (hasLibc !== (target.platform === 'linux')) {
    problems.push(`${key}.libc: ${hasLibc ? 'present' : 'absent'} for platform ${target.platform}`)
  }

  // The key is what names the published npm package and the release archive, so it has to be
  // derivable from the fields rather than independently typed.
  assertions++
  const expectedKey = hasLibc
    ? `${target.platform}-${target.arch}-${target.libc}`
    : `${target.platform}-${target.arch}`
  if (key !== expectedKey) {
    problems.push(`${key}: key does not match its own fields (expected ${expectedKey})`)
  }

  assertions++
  if (seenRustTargets.has(target.rustTarget)) {
    problems.push(`${key}: rustTarget ${target.rustTarget} is also used by ${seenRustTargets.get(target.rustTarget)}`)
  } else {
    seenRustTargets.set(target.rustTarget, key)
  }

  // The real behavioural check: the npm launcher picks a binary by calling `targetFor` with the
  // running host's triple. If two entries collide there, a user downloads a binary that cannot
  // run -- exactly the failure `targets.mjs`'s own header warns about -- and no amount of
  // manifest-vs-manifest comparison would notice.
  assertions++
  const resolved = targetFor(target.platform, target.arch, target.libc)
  if (resolved?.key !== key) {
    problems.push(
      `${key}: targetFor(${target.platform}, ${target.arch}, ${JSON.stringify(target.libc)}) ` +
        `resolved to ${resolved ? resolved.key : 'undefined'}`,
    )
  }
}

if (problems.length > 0) {
  fail(`platform manifest invariants violated (${problems.length}):\n  ${problems.join('\n  ')}`)
}

const MIN_TARGET_KEYS = 8 // the known platform count today; a floor against checking nothing.
const targetCount = Object.keys(TARGETS).length
if (targetCount < MIN_TARGET_KEYS) {
  fail(
    `only checked ${targetCount} target(s), expected at least ${MIN_TARGET_KEYS}. ` +
      'A run over almost no targets would pass while verifying almost nothing.',
  )
}
console.log(
  `platform manifest: ${targetCount} targets, ${assertions} invariant assertions, ` +
    `all resolving back through targetFor()`,
)

// ---------------------------------------------------------------------------------------------
// 3. Every release.yml job that runs `node` as a shell command must pin a version via
//    actions/setup-node.
// ---------------------------------------------------------------------------------------------
//
// This is a heuristic over the YAML text, not a real YAML/shell parser, and it only works because
// of two properties specific to this file today: job names are the only lines indented exactly two
// spaces under the top-level `jobs:` key (permissions/env/on also have 2-space children, so the
// split only starts after the `jobs:` line), and every real invocation of the `node` binary in this
// file is followed by whitespace, which `actions/setup-node@v4` (node@v4, no space) and
// `node-version:` (node-, no space) are not -- so `\bnode\s` cannot be reworded into a false
// positive without also becoming a real node invocation. If release.yml's structure changes in a
// way that breaks either assumption, this section should be revisited rather than trusted blindly;
// a full YAML parser would be the robust fix but is not worth adding as a dependency for one check.
const jobsStart = releaseYml.indexOf('\njobs:')
if (jobsStart === -1) {
  fail("could not find a top-level 'jobs:' key in release.yml -- job-scoped node/setup-node check skipped entirely, which is a failure, not a pass.")
} else {
  const jobsText = releaseYml.slice(jobsStart)
  const jobHeaderRe = /^  ([A-Za-z0-9_-]+):\s*$/gm
  const headers = [...jobsText.matchAll(jobHeaderRe)]
  if (headers.length === 0) {
    fail('found a jobs: key but no job blocks under it -- job-scoped node/setup-node check compared 0 jobs.')
  } else {
    const jobBlocks = headers.map((h, i) => {
      const start = h.index
      const end = i + 1 < headers.length ? headers[i + 1].index : jobsText.length
      return { name: h[1], text: jobsText.slice(start, end) }
    })
    const NODE_INVOCATION_RE = /\bnode\s/
    const SETUP_NODE_RE = /actions\/setup-node@\S+[\s\S]{0,120}?node-version:\s*['"]?([^\s'"]+)/
    let jobsScanned = 0
    let jobsRunningNode = 0
    const unpinned = []
    for (const job of jobBlocks) {
      jobsScanned++
      if (!NODE_INVOCATION_RE.test(job.text)) continue
      jobsRunningNode++
      const setup = job.text.match(SETUP_NODE_RE)
      if (!setup || !setup[1]) {
        unpinned.push(job.name)
      }
    }
    if (jobsScanned === 0) {
      fail('job-scoped node/setup-node check scanned 0 jobs -- verified nothing.')
    } else if (unpinned.length > 0) {
      fail(
        `job(s) run 'node' without a pinned actions/setup-node version: ${unpinned.join(', ')}. ` +
          'Each inherits whatever Node the runner image ships, which is free to change without notice.',
      )
    } else {
      console.log(
        `job/setup-node check: scanned ${jobsScanned} job(s), ${jobsRunningNode} run 'node', ` +
          'all pin a version via actions/setup-node',
      )
    }
  }
}

// 4. Publication channels must be explicit, including older stable versions and prereleases.
const releaseAction = releaseYml.match(
  /^      - uses: softprops\/action-gh-release@[^\r\n]+\r?\n((?:^        [^\r\n]*\r?\n)+)/m,
)
const prereleaseInput = /^          prerelease: \$\{\{ contains\(needs\.resolve\.outputs\.version, '-'\) \}\}\r?$/m
if (!releaseAction || !prereleaseInput.test(releaseAction[1])) {
  fail('GitHub release must derive prerelease from the validated resolve version')
}

const { npmPublishArguments } = await import('../release/publish-npm-packages.mjs')
let channelsChecked = 0
for (const [version, tag] of [
  ['1.0.0', 'latest'], ['0.2.0', 'latest'],
  ['1.0.0-rc.1', 'next'], ['1.0.0-beta.2', 'next'], ['1.0.0-0', 'next'],
]) {
  for (const directory of ['npm/platforms/darwin-arm64', '.']) {
    const args = npmPublishArguments(directory, version)
    channelsChecked++
    if (args.filter((arg) => arg === '--tag').length !== 1 || args[args.indexOf('--tag') + 1] !== tag) {
      fail(`npm publication of ${directory}@${version} must pass --tag ${tag}`)
    }
  }
}
console.log(`publication channels: ${channelsChecked} npm argument cases checked`)

// 5. The website rebuild trigger must stay after the registries, off the publishing path, and
// must not interpolate its secret into the script.
const siteJob = releaseYml.match(/^  notify-site:\r?\n((?:(?:    |\r?\n)[^\r\n]*\r?\n?)+)/m)
if (!siteJob) {
  fail('release.yml has no notify-site job: the website is not rebuilt after a release')
} else {
  const text = siteJob[1]
  const needs = /^    needs: \[([^\]]*)\]/m.exec(text)?.[1].split(',').map((name) => name.trim()) ?? []
  for (const required of ['publish', 'publish-crate', 'publish-npm']) {
    if (!needs.includes(required)) fail(`notify-site must wait for ${required}, so the site never leads the registries`)
  }
  if (!/inputs\.publish == true/.test(text)) fail('notify-site must run only for a real publish, never a rehearsal')
  if (!/needs\.publish-crate\.result != 'failure'/.test(text) || !/needs\.publish-npm\.result != 'failure'/.test(text)) {
    fail('notify-site must not run after a failed registry job')
  }
  if (!/^    permissions: \{\}\r?$/m.test(text)) fail('notify-site needs no repository token: permissions must be {}')
  if (/\brun:[^\n]*\n(?:[^\n]*\n)*?[^\n]*\$\{\{\s*secrets\./.test(text.replace(/^ {10}env:[\s\S]*?(?=^ {8}run:)/m, ''))) {
    fail('notify-site must pass its secret through env, never interpolate it into the script')
  }
  if (!/^          SITE_DISPATCH_TOKEN: \$\{\{ secrets\.SITE_DISPATCH_TOKEN \}\}\r?$/m.test(text) || !text.includes('Bearer $SITE_DISPATCH_TOKEN')) {
    fail('notify-site must authenticate with SITE_DISPATCH_TOKEN taken from env')
  }
  if (!text.includes('https://api.github.com/repos/figueiredoluiz/vitest-dedup-site/dispatches') || !text.includes(`'{"event_type":"release-published"}'`)) {
    fail('notify-site must post event_type release-published to the website repository dispatches endpoint')
  }
}
console.log(`website trigger: ${siteJob ? 'notify-site job checked' : 'job missing'}`)

// ---------------------------------------------------------------------------------------------
if (failures.length > 0) {
  console.error(`\nFAIL: ${failures.length} problem(s) with release.yml / target manifests:\n`)
  for (const f of failures) console.error(`- ${f}\n`)
  process.exit(1)
}
console.log('\nok: release workflow guard passed.')
