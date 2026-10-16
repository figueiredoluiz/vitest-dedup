import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { parseDocument } from 'yaml'
import { commands } from '../../scripts/dev/verify.mjs'
import { runCli } from '../../scripts/check/check-action-pins.mjs'

const sha = 'a'.repeat(40)
/** Run the real gate over one owned workflow, cleaning up every fixture. */
function check(source, args = [], filename = 'check.yml') {
  const root = mkdtempSync(join(tmpdir(), 'vitest-dedup-action-pins-'))
  try {
    const directory = join(root, '.github', 'workflows')
    mkdirSync(directory, { recursive: true })
    if (source !== null) writeFileSync(join(directory, filename), source)
    return runCli(args, { root })
  } finally { rmSync(root, { recursive: true, force: true }) }
}
/** A workflow that selects the supplied action reference. */
const workflow = reference => `on: push\njobs:\n  check:\n    steps:\n      - uses: ${reference}\n`

test('full SHA pins with version comments pass; local actions and shell prose are not remote code', () => {
  const result = check(`${workflow(`'actions/checkout@${sha}' # v7`)}      - uses: ./\n      - run: 'echo uses: mutable/action@main'\n`)
  assert.equal(result.status, 0, result.lines.join('\n'))
  assert.match(result.lines[0], /1 workflows, 1 remote references/)
})

for (const reference of ['actions/checkout@v7', 'actions/checkout@main', 'actions/checkout@aaaaaaa',
  `actions/checkout@${'g'.repeat(40)}`, '${{ inputs.action }}', '42', 'null', '[]', '{}', 'true']) {
  test(`rejects unpinned or non-string uses ${reference}`, () => {
    const result = check(workflow(reference))
    assert.equal(result.status, 1)
    assert.match(result.lines.join('\n'), /remote uses must select/)
  })
}

test('a version comment is required for a full pin', () => {
  assert.match(check(workflow(`actions/checkout@${sha}`)).lines.join('\n'), /needs a comment/)
})

test('flow mappings and CRLF are parsed, including reusable workflow job uses', () => {
  const source = `jobs:\r\n  build:\r\n    uses: org/repo/.github/workflows/build.yml@${sha} # v1\r\n  check:\r\n    steps: [{uses: 'actions/checkout@main'}]\r\n`
  const result = check(source, [], 'check.yaml')
  assert.equal(result.status, 1)
  assert.match(result.lines[0], /2 remote references/)
  assert.match(result.lines.join('\n'), /check:step 1/)
})

test('Docker digests pass but Docker tags fail', () => {
  assert.equal(check(workflow(`docker://example/tool@sha256:${'a'.repeat(64)} # v1`)).status, 0)
  assert.equal(check(workflow('docker://example/tool:latest')).status, 1)
})

for (const source of [null, '', 'jobs: {}', 'jobs: [', 'jobs: {one: null}',
  'jobs: {one: {steps: {}}}', 'jobs: {one: {steps: [null]}}',
  'jobs: {one: {runs-on: ubuntu-latest}}', 'jobs: {one: {steps: [{uses: ./}]}}']) {
  test(`rejects empty or invalid workflow input ${JSON.stringify(source)}`, () => {
    const result = check(source)
    assert.equal(result.status, 1)
    assert.ok(result.lines.length > 1)
  })
}

test('unsupported CLI arguments fail before scanning', () => {
  assert.match(check(null, ['--skip']).lines.join('\n'), /Usage:/)
})

test('the repository has a non-empty, fully pinned workflow set', () => {
  const result = runCli()
  assert.equal(result.status, 0, result.lines.join('\n'))
})


test('workflow consumers install the YAML parser and local gates enforce pins', () => {
  const ci = parseDocument(readFileSync(new URL('../../.github/workflows/ci.yml', import.meta.url), 'utf8')).toJS()
  for (const [job, command] of [['lint', 'npm run check:action-pins'], ['npm-dist', 'npm test']]) {
    const steps = ci.jobs[job].steps
    const install = steps.findIndex(step => step.run === 'npm ci --force --ignore-scripts --no-audit --no-fund')
    const consume = steps.findIndex(step => step.run === command)
    assert.ok(install >= 0 && consume > install, `${job} installs dependencies before ${command}`)
  }
  assert.ok(commands('fast', false).some(step => step.join(' ') === 'npm run check:action-pins'))
  const manifest = JSON.parse(readFileSync(new URL('../../package.json', import.meta.url), 'utf8'))
  assert.match(manifest.scripts.check, /npm run check:action-pins/)
})


test('checkout credentials and privileged package caches are disabled', () => {
  const directory = new URL('../../.github/workflows/', import.meta.url)
  for (const file of readdirSync(directory).filter(name => /\.ya?ml$/.test(name))) {
    const workflow = parseDocument(readFileSync(new URL(file, directory), 'utf8')).toJS()
    for (const job of Object.values(workflow.jobs)) {
      for (const step of job.steps ?? []) {
        if (step.uses?.startsWith('actions/checkout@')) {
          assert.equal(step.with?.['persist-credentials'], false, `${file}: checkout credentials`)
        }
        if (step.uses?.startsWith('actions/setup-node@') && job.permissions?.['id-token'] === 'write') {
          assert.equal(step.with?.['package-manager-cache'], false, `${file}: privileged cache`)
        }
      }
    }
  }
})
