// The release's corpus comparison, driven with a fake `gh`/`tar`/evaluator so every refusal is
// reached without a network. Each failure mode here is one that must stop publication rather than
// read as a comparison that was made.
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import {
  apply,
  archiveName,
  compare,
  END_MARKER,
  expectedDigest,
  literal,
  MAX_NOTES_LENGTH,
  parseArguments,
  renderSection,
  runCli,
  selectPreviousRelease,
  START_MARKER,
  upsertSection,
} from '../../scripts/release/release-comparison.mjs'

const release = (tagName, extra = {}) => ({ tagName, isPrerelease: false, isDraft: false, ...extra })

test('the previous release is the newest published one that is not this release', () => {
  const list = [release('v0.6.0-rc.1', { isPrerelease: true }), release('v0.5.1'), release('v0.5.0')]
  assert.equal(selectPreviousRelease(list, '0.6.0'), 'v0.5.1', 'a stable release skips a prerelease')
  assert.equal(selectPreviousRelease(list, '0.6.0-rc.2'), 'v0.6.0-rc.1', 'a prerelease may follow a prerelease')
  assert.equal(selectPreviousRelease([release('v0.6.0'), release('v0.5.1')], '0.6.0'), 'v0.5.1', 'not itself')
  assert.equal(selectPreviousRelease([release('v1.0.0-rc.1', { isPrerelease: true })], '1.0.0'), 'v1.0.0-rc.1', 'a prerelease stands in when no stable release exists')
  assert.equal(selectPreviousRelease([release('v1.0.0-rc.2', { isPrerelease: true }), release('v0.9.0')], '1.0.0'), 'v0.9.0', 'but never ahead of a stable one')
  assert.equal(selectPreviousRelease([release('v0.5.1', { isDraft: true })], '0.6.0'), null, 'drafts are not published')
  assert.equal(selectPreviousRelease([release('nightly')], '0.6.0'), null, 'only version tags')
  assert.equal(selectPreviousRelease([], '0.1.0'), null)
})

test('a checksum file must name the archive it vouches for', () => {
  const digest = 'a'.repeat(64)
  assert.equal(expectedDigest(`${digest}  vitest-dedup-0.5.1-linux-x64-gnu.tar.gz\n`, 'vitest-dedup-0.5.1-linux-x64-gnu.tar.gz'), digest)
  assert.equal(expectedDigest(`${digest} *dist/x.tar.gz`, 'x.tar.gz'), digest)
  assert.throws(() => expectedDigest(`${digest}  other.tar.gz`, 'x.tar.gz'), /does not record a sha256 for x\.tar\.gz/)
  assert.throws(() => expectedDigest('not a digest', 'x.tar.gz'), /does not record/)
})

/** The comparison object the evaluator writes, with the given regression counts. */
function comparison({ regressed = 0, flagged = [] } = {}) {
  const tally = { retained: 211, 'newly-satisfied': 2, corrected: 5, regressed, 'new-forbidden': 0, 'still-unmet': 0, unavailable: 0 }
  return {
    totals: { detector: tally, labelChanged: tally, newUnclaimed: 0, goneUnclaimed: 0, extractionRegressed: 0 },
    unavailableCases: 0,
    labelChangedCases: [{ name: 'new-case', reason: 'new' }],
    cases: flagged.map((name) => ({ name, pending: null, labelChange: null, newUnclaimed: [], extraction: 'retained', expectations: [{ class: 'regressed' }] })),
  }
}

test('the section states the outcome classes, the corpus size and what the counts are not', () => {
  const section = renderSection({ comparison: comparison(), version: '0.6.0', previousTag: 'v0.5.1', cases: 119 })
  assert.ok(section.startsWith(START_MARKER) && section.trimEnd().endsWith(END_MARKER))
  assert.match(section, /v0\.6\.0 against v0\.5\.1 on the 119-case recall corpus/)
  assert.match(section, /\| Expectations retained \| 211 \|/)
  assert.match(section, /\*\*Expectations regressed\*\* \(a wanted finding lost\) \| 0 \|/)
  assert.match(section, /Cases new or changed since v0\.5\.1, counted apart \| 1 \|/)
  assert.match(section, /not a measure of precision on real code/)
  assert.match(section, /\| Outcome \| Count \|/)
  assert.match(section, /Cases whose extraction regressed/)
  assert.match(section, /Cases v0\.5\.1 could not be run on/)
  assert.match(section, /New findings the labels do not mention/)
  assert.doesNotMatch(section, /Cases with a regression/)
})

test('the section names the target it was built for', () => {
  const section = renderSection({ comparison: comparison(), version: '0.6.0', previousTag: 'v0.5.1', cases: 119, key: 'darwin-arm64' })
  assert.match(section, /built for darwin-arm64\./)
})

test('regressed cases are named, with a cap', () => {
  const many = Array.from({ length: 23 }, (_, index) => `case-${index}`)
  const section = renderSection({ comparison: comparison({ regressed: 23, flagged: many }), version: '0.6.0', previousTag: 'v0.5.1', cases: 119 })
  assert.match(section, /Cases with a regression \(23\)/)
  assert.match(section, /- `case-0`/)
  assert.doesNotMatch(section, /case-20/)
  assert.match(section, /and 3 more, in the artifact/)
})

test('a case name is written as literal text and cannot plant a second section', () => {
  const hostile = `evil\`\n${END_MARKER}\n${START_MARKER}\n## Recall corpus comparison`
  assert.doesNotMatch(literal(hostile), /[`\n<>]/)
  assert.equal(literal('duplicate-test-in-source'), 'duplicate-test-in-source')
  assert.equal(literal('x'.repeat(300)).length, 120)
  const section = renderSection({ comparison: comparison({ regressed: 1, flagged: [hostile] }), version: '0.6.0', previousTag: 'v0.5.1', cases: 119 })
  assert.equal(section.split(START_MARKER).length - 1, 1)
  assert.equal(section.split(END_MARKER).length - 1, 1)
  assert.equal(upsertSection(upsertSection('notes', section), section).split(START_MARKER).length - 1, 1)
})

test('with no earlier release the section says so rather than staying silent', () => {
  const section = renderSection({ comparison: null, version: '0.1.0', previousTag: null, cases: 0 })
  assert.match(section, /no earlier release of vitest-dedup to compare v0\.1\.0 with/)
  assert.doesNotMatch(section, /\| Expectations retained/)
})

test('the section replaces an earlier one, so a retried publish keeps exactly one', () => {
  const first = renderSection({ comparison: comparison(), version: '0.6.0', previousTag: 'v0.5.1', cases: 119 })
  const second = renderSection({ comparison: comparison({ regressed: 1, flagged: ['c'] }), version: '0.6.0', previousTag: 'v0.5.1', cases: 119 })
  const generated = '## What\'s Changed\n* a change\n\n**Full Changelog**: x'
  const once = upsertSection(generated, first)
  assert.ok(once.includes(generated), 'the generated notes are kept')
  assert.equal(upsertSection(once, first), once, 'applying the same section again changes nothing')
  const replaced = upsertSection(once, second)
  assert.equal(replaced.split(START_MARKER).length - 1, 1)
  assert.match(replaced, /Cases with a regression/)
  assert.doesNotMatch(replaced, /\*\*Expectations regressed\*\* \(a wanted finding lost\) \| 0/)
  assert.ok(replaced.startsWith(START_MARKER), 'the section leads the notes')
  assert.equal(upsertSection(null, first), first, 'an empty body is just the section')
  const doubled = upsertSection(`${first}\n${first}\n${generated}`, second)
  assert.equal(doubled.split(START_MARKER).length - 1, 1, 'a body that somehow holds two collapses to one')
})

test('notes that would exceed the size limit give way to the section, once, however often it is applied', () => {
  const section = renderSection({ comparison: null, version: '0.1.0', previousTag: null, cases: 0 })
  const huge = 'x'.repeat(MAX_NOTES_LENGTH - 10)
  const once = upsertSection(huge, section)
  assert.ok(once.length <= MAX_NOTES_LENGTH, `${once.length} characters`)
  assert.ok(once.startsWith(START_MARKER) && once.includes('cut to fit'))
  assert.equal(upsertSection(once, section), once, 'a retry changes nothing')
  assert.equal(once.split('cut to fit').length - 1, 1)
  const fits = 'y'.repeat(1000)
  assert.ok(!upsertSection(fits, section).includes('cut to fit'), 'notes that fit are left whole')
})

/** A fake command runner keyed by program and first argument. */
function fakeRun(handlers, calls = []) {
  const execute = (command, args, options) => {
    calls.push([command, args, options])
    const key = command === process.execPath ? 'node' : command
    const handler = handlers[`${key} ${args[0]}`] ?? handlers[key]
    if (!handler) throw new Error(`unexpected command ${key} ${args.join(' ')}`)
    return handler(args, options)
  }
  return { execute, calls }
}

const ok = (stdout = '') => ({ status: 0, stdout, stderr: '' })
const failed = (stderr = 'boom') => ({ status: 1, stdout: '', stderr })

/** Handlers for a healthy comparison against v0.5.1. */
function healthy(overrides = {}, { tamper = false } = {}) {
  const archive = Buffer.from('archive bytes')
  return {
    'gh release': (args) => {
      if (args[1] === 'list') return ok(JSON.stringify([release('v0.5.1')]))
      const dir = args[args.indexOf('--dir') + 1]
      writeFileSync(join(dir, archiveName('0.5.1')), tamper ? Buffer.from('other bytes') : archive)
      writeFileSync(
        join(dir, `${archiveName('0.5.1')}.sha256`),
        `${createHash('sha256').update(archive).digest('hex')}  ${archiveName('0.5.1')}\n`,
      )
      return ok()
    },
    'gh attestation': () => ok(),
    tar: () => ok(),
    node: (args) => {
      writeFileSync(args[args.indexOf('--json') + 1], JSON.stringify({ comparison: comparison(), work: { cases: 119 } }))
      return ok()
    },
    ...overrides,
  }
}

const base = () => ({ version: '0.6.0', binary: 'native/vitest-dedup', out: join(mkdtempSync(join(tmpdir(), 'cmp-out-')), 'out'), repo: 'o/r' })

test('a healthy comparison writes the section, the markdown and the full detail', () => {
  const options = base()
  const { execute, calls } = fakeRun(healthy())
  const result = compare(options, { execute, root: '/checkout' })
  assert.equal(result.status, 'compared')
  assert.equal(result.previousTag, 'v0.5.1')
  for (const file of ['notes-section.md', 'comparison.md', 'comparison.json']) assert.ok(existsSync(join(options.out, file)), file)
  assert.match(readFileSync(join(options.out, 'notes-section.md'), 'utf8'), /v0\.6\.0 against v0\.5\.1 on the 119-case recall corpus/)
  assert.equal(JSON.parse(readFileSync(join(options.out, 'comparison.json'), 'utf8')).previousTag, 'v0.5.1')
  const evaluator = calls.find(([command]) => command === process.execPath)
  const argumentsOf = evaluator[1]
  const after = (flag) => argumentsOf[argumentsOf.indexOf(flag) + 1]
  assert.match(argumentsOf[1], /native[\\/]vitest-dedup$/, 'the candidate is the built binary')
  assert.match(after('--baseline-binary'), /vitest-dedup-0\.5\.1-linux-x64-gnu[\\/]vitest-dedup$/)
  assert.equal(after('--baseline-ref'), 'v0.5.1')
  assert.equal(evaluator[2].cwd, '/checkout')
  const verify = calls.findIndex(([command, args]) => command === 'gh' && args[0] === 'attestation')
  const extract = calls.findIndex(([command]) => command === 'tar')
  const evaluate = calls.findIndex(([command]) => command === process.execPath)
  assert.ok(verify > -1 && verify < extract && extract < evaluate, 'the archive is verified before it is unpacked and run')
})

test('a target other than the default is carried through the download and the section', () => {
  const options = { ...base(), key: 'darwin-arm64' }
  const { execute, calls } = fakeRun(healthy({ 'gh release': (args) => {
    if (args[1] === 'list') return ok(JSON.stringify([release('v0.5.1')]))
    const dir = args[args.indexOf('--dir') + 1]
    const archive = Buffer.from('x')
    writeFileSync(join(dir, archiveName('0.5.1', 'darwin-arm64')), archive)
    writeFileSync(join(dir, `${archiveName('0.5.1', 'darwin-arm64')}.sha256`), `${createHash('sha256').update(archive).digest('hex')}  ${archiveName('0.5.1', 'darwin-arm64')}\n`)
    return ok()
  } }))
  compare(options, { execute, root: '/checkout' })
  assert.ok(calls.some(([, args]) => args.includes(archiveName('0.5.1', 'darwin-arm64'))))
  assert.match(readFileSync(join(options.out, 'notes-section.md'), 'utf8'), /built for darwin-arm64/)
  const sections = parseArguments(['compare', '--version', '1.0.0', '--binary', 'b', '--out', 'o', '--target', 'darwin-arm64'])
  assert.equal(sections.options.target, 'darwin-arm64')
})

test('no earlier release is recorded as that, with no download and no evaluation', () => {
  const options = base()
  const { execute, calls } = fakeRun({ 'gh release': () => ok('[]') })
  const result = compare(options, { execute })
  assert.equal(result.status, 'no-previous')
  assert.equal(calls.length, 1)
  assert.match(readFileSync(join(options.out, 'notes-section.md'), 'utf8'), /no earlier release/)
  assert.equal(JSON.parse(readFileSync(join(options.out, 'comparison.json'), 'utf8')).comparison, null)
})

test('a full page of releases with none usable is a failure, and drafts are not listed', () => {
  const options = base()
  const page = Array.from({ length: 100 }, (_, index) => release(`nightly-${index}`))
  const { execute, calls } = fakeRun({ 'gh release': () => ok(JSON.stringify(page)) })
  assert.throws(() => compare(options, { execute }), /older ones were not listed/)
  assert.equal(existsSync(join(options.out, 'notes-section.md')), false)
  assert.ok(calls[0][1].includes('--exclude-drafts'))
  const short = fakeRun({ 'gh release': () => ok(JSON.stringify(page.slice(0, 99))) })
  assert.equal(compare(base(), { execute: short.execute }).status, 'no-previous', 'a short list does establish that there is none')
})

test('anything that stops the comparison from being made fails instead of writing a section', () => {
  const refusal = (name, handlers, pattern, options = {}) => {
    const settings = base()
    assert.throws(() => compare(settings, { execute: fakeRun(handlers).execute, ...options }), pattern, name)
    assert.equal(existsSync(join(settings.out, 'notes-section.md')), false, `${name}: no section is written`)
  }
  refusal('release list fails', healthy({ 'gh release': () => failed('no network') }), /gh release list/)
  refusal('archive altered', healthy({}, { tamper: true }), /does not match its published sha256/)
  refusal('attestation rejected', healthy({ 'gh attestation': () => failed('no attestation') }), /gh attestation verify/)
  refusal('tar fails', healthy({ tar: () => failed('bad archive') }), /tar -xzf/)
  refusal('evaluator fails', healthy({ node: () => failed('exited 2') }), /evaluate-corpus\.mjs/)
  refusal('evaluation has no comparison', healthy({ node: (args) => {
    writeFileSync(args[args.indexOf('--json') + 1], JSON.stringify({ work: { cases: 1 } }))
    return ok()
  } }), /produced no comparison/)
  refusal('download fails', healthy({ 'gh release': (args) => (args[1] === 'list' ? ok(JSON.stringify([release('v0.5.1')])) : failed('404')) }), /gh release download/)
})

test('apply writes the section into the notes once and refuses a file that is not a section', () => {
  const dir = mkdtempSync(join(tmpdir(), 'apply-'))
  const sectionFile = join(dir, 'notes-section.md')
  const section = renderSection({ comparison: comparison(), version: '0.6.0', previousTag: 'v0.5.1', cases: 119 })
  writeFileSync(sectionFile, section)
  let notes = "## What's Changed\n* a change\n"
  const written = []
  const { execute } = fakeRun({
    'gh release': (args) => {
      if (args[1] === 'view') return ok(JSON.stringify({ body: notes }))
      const file = args[args.indexOf('--notes-file') + 1]
      notes = readFileSync(file, 'utf8')
      written.push(notes)
      return ok()
    },
  })
  apply({ tag: 'v0.6.0', sectionFile, repo: 'o/r' }, { execute })
  apply({ tag: 'v0.6.0', sectionFile, repo: 'o/r' }, { execute })
  assert.equal(written.length, 2)
  assert.equal(written[0], written[1], 'a second attempt rewrites the same notes')
  assert.equal(written[1].split(START_MARKER).length - 1, 1)
  assert.ok(written[1].includes('* a change'))
  writeFileSync(join(dir, 'plain.md'), 'just text')
  assert.throws(() => apply({ tag: 'v0.6.0', sectionFile: join(dir, 'plain.md'), repo: 'o/r' }, { execute }), /is not a comparison section/)
  assert.throws(() => apply({ tag: 'v0.6.0', sectionFile, repo: 'o/r' }, { execute: fakeRun({ 'gh release': () => failed('gone') }).execute }), /gh release view/)
})

test('arguments name one of two commands and every option it needs', () => {
  assert.deepEqual(parseArguments(['compare', '--version', '0.6.0', '--binary', 'b', '--out', 'o']).options, { version: '0.6.0', binary: 'b', out: 'o' })
  assert.deepEqual(parseArguments(['apply', '--tag', 'v0.6.0', '--section', 's']).options, { tag: 'v0.6.0', section: 's' })
  assert.throws(() => parseArguments([]), /usage/)
  assert.throws(() => parseArguments(['publish']), /usage/)
  assert.throws(() => parseArguments(['apply', '--tag', 'v1.0.0']), /missing --section/)
  assert.throws(() => parseArguments(['apply', '--tag', 'v1.0.0', '--section']), /incomplete argument/)
  assert.throws(() => parseArguments(['apply', '--tag', '', '--section', 's']), /incomplete argument/)
  assert.throws(() => parseArguments(['apply', '--tag', 'v1.0.0', '--bogus', 'x']), /unexpected or incomplete argument --bogus/)
  assert.throws(() => parseArguments(['compare', '--version', '1.2', '--binary', 'b', '--out', 'o']), /--version must look like/)
  assert.throws(() => parseArguments(['compare', '--version', '1.2.3 <!-- x -->', '--binary', 'b', '--out', 'o']), /--version must look like/)
  assert.throws(() => parseArguments(['apply', '--tag', '1.2.3', '--section', 's']), /--tag must look like v1.2.3/)
  assert.equal(parseArguments(['compare', '--version', '1.2.3-rc.1', '--binary', 'b', '--out', 'o']).options.version, '1.2.3-rc.1')
})

test('the command line needs a repository and reports what it did', () => {
  const dir = mkdtempSync(join(tmpdir(), 'cli-'))
  const sectionFile = join(dir, 's.md')
  writeFileSync(sectionFile, renderSection({ comparison: null, version: '0.1.0', previousTag: null, cases: 0 }))
  assert.throws(() => runCli(['apply', '--tag', 'v0.1.0', '--section', sectionFile], {}), /GH_REPO is required/)
  const { execute } = fakeRun({ 'gh release': (args) => (args[1] === 'view' ? ok(JSON.stringify({ body: '' })) : ok()) })
  assert.match(runCli(['apply', '--tag', 'v0.1.0', '--section', sectionFile], { GITHUB_REPOSITORY: 'o/r' }, { execute }).lines[0], /applied to v0\.1\.0/)
  const out = join(dir, 'out')
  mkdirSync(out)
  const compared = runCli(['compare', '--version', '0.1.0', '--binary', 'b', '--out', out], { GH_REPO: 'o/r' }, { execute: fakeRun({ 'gh release': () => ok('[]') }).execute })
  assert.equal(compared.lines[0], 'Comparison no-previous.')
})
