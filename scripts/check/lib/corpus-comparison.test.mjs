// Tests for the two-binary comparison. Every class is driven with fabricated evaluations, and the
// classification is pinned for each kind of expectation in both directions so a swapped pair of
// classes (a lost finding read as a gain) cannot pass.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'

import {
  binaryVersion,
  buildRunRecord,
  caseDigest,
  CHANGE_CLASSES,
  classifyChange,
  compareCase,
  compareCorpus,
  corpusDigest,
  readLabelChanges,
  regressedCases,
  renderComparison,
  runGit,
} from './corpus-comparison.mjs'

/** Good and bad status for each expectation kind. */
const STATUS = {
  'expected-finding': { good: 'satisfied', bad: 'missing' },
  'expected-absent': { good: 'held', bad: 'violated' },
  'known-miss': { good: 'now-reported', bad: 'still-missed' },
  'known-false-positive': { good: 'no-longer-reported', bad: 'still-reported' },
}

test('each kind is classed by what changed, in both directions', () => {
  const expected = {
    'expected-finding': { gained: 'newly-satisfied', lost: 'regressed' },
    'expected-absent': { gained: 'corrected', lost: 'new-forbidden' },
    'known-miss': { gained: 'newly-satisfied', lost: 'regressed' },
    'known-false-positive': { gained: 'corrected', lost: 'new-forbidden' },
  }
  for (const [kind, { good, bad }] of Object.entries(STATUS)) {
    assert.equal(classifyChange(kind, good, good), 'retained', kind)
    assert.equal(classifyChange(kind, bad, bad), 'still-unmet', kind)
    assert.equal(classifyChange(kind, bad, good), expected[kind].gained, kind)
    assert.equal(classifyChange(kind, good, bad), expected[kind].lost, kind)
  }
  assert.throws(() => classifyChange('invented', 'a', 'b'), /unknown expectation kind/)
  assert.equal(new Set(CHANGE_CLASSES).size, CHANGE_CLASSES.length)
})

/** An evaluated case with the given expectation statuses and unclaimed findings. */
function evaluated(name, statuses, unclaimed = [], extra = {}) {
  const kinds = Object.keys(STATUS)
  return {
    name,
    rule: 'duplicate-test',
    pending: null,
    unclaimed,
    extraction: { matches: true },
    expectations: statuses.map((status, index) => ({ id: `${name}#${index + 1}`, kind: kinds[index % kinds.length], rule: 'duplicate-test', status })),
    ...extra,
  }
}

const unclaimedFinding = (line) => ({ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'], locations: [`a.test.ts:${line}`] })

test('a case is compared expectation by expectation and by unclaimed findings', () => {
  const baseline = evaluated('c', ['satisfied', 'held', 'still-missed'], [unclaimedFinding(1), unclaimedFinding(2)])
  const candidate = evaluated('c', ['missing', 'held', 'now-reported'], [unclaimedFinding(2), unclaimedFinding(3)])
  const result = compareCase(candidate, { evaluated: baseline }, null)
  assert.deepEqual(result.expectations.map((o) => o.class), ['regressed', 'retained', 'newly-satisfied'])
  assert.deepEqual(result.newUnclaimed.map((f) => f.locations[0]), ['a.test.ts:3'])
  assert.deepEqual(result.goneUnclaimed.map((f) => f.locations[0]), ['a.test.ts:1'])
  assert.throws(
    () => compareCase(candidate, { evaluated: evaluated('c', ['satisfied']) }, null),
    /expectation c#2 is missing from the baseline/,
  )
})

test('a case the baseline could not be evaluated on is unavailable, never zero findings', () => {
  const result = compareCase(evaluated('c', ['satisfied', 'held']), { unavailable: 'binary exited 2' }, null)
  assert.equal(result.unavailable, 'binary exited 2')
  assert.deepEqual(result.expectations.map((o) => o.class), ['unavailable', 'unavailable'])
})

/** Compares prepared pairs of evaluations. */
function compare(pairs, labelChanges = new Map()) {
  return compareCorpus(
    pairs.map(([candidate]) => candidate),
    pairs.map(([, baseline]) => baseline),
    labelChanges,
  )
}

test('totals separate detector changes from label changes and leave pending cases out', () => {
  const lost = [evaluated('lost', ['missing']), { evaluated: evaluated('lost', ['satisfied']) }]
  const gained = [evaluated('gained', ['satisfied']), { evaluated: evaluated('gained', ['missing']) }]
  const relabelled = [evaluated('relabelled', ['missing']), { evaluated: evaluated('relabelled', ['satisfied']) }]
  const pending = [evaluated('held', ['missing'], [], { pending: 'waits on a named defect' }), { evaluated: evaluated('held', ['satisfied']) }]
  const out = compare([lost, gained, relabelled, pending], new Map([['relabelled', 'manifest entry differs']]))
  assert.equal(out.totals.detector.regressed, 1)
  assert.equal(out.totals.detector['newly-satisfied'], 1)
  assert.equal(out.totals.labelChanged.regressed, 1, 'a regression on a relabelled case is not a detector regression')
  assert.equal(out.regressions, 1)
  assert.equal(out.cases.length, 4, 'the pending case is listed')
  assert.deepEqual(out.labelChangedCases, [{ name: 'relabelled', reason: 'manifest entry differs' }])
  assert.deepEqual(out.byRule.map((r) => [r.rule, r.regressed]), [['duplicate-test', 1]])
})

test('new unclaimed findings and new forbidden findings count as regressions', () => {
  const unclaimed = [evaluated('u', ['satisfied'], [unclaimedFinding(5)]), { evaluated: evaluated('u', ['satisfied']) }]
  assert.equal(compare([unclaimed]).regressions, 1)
  const forbidden = [evaluated('f', ['satisfied', 'violated']), { evaluated: evaluated('f', ['satisfied', 'held']) }]
  assert.equal(compare([forbidden]).totals.detector['new-forbidden'], 1)
  assert.equal(compare([forbidden]).regressions, 1)
  const clean = [evaluated('k', ['satisfied', 'held']), { evaluated: evaluated('k', ['satisfied', 'held']) }]
  assert.equal(compare([clean]).regressions, 0)
})

test('unavailable cases are counted and a baseline that ran on none is an error', () => {
  const ok = [evaluated('ok', ['satisfied']), { evaluated: evaluated('ok', ['satisfied']) }]
  const down = [evaluated('down', ['satisfied']), { unavailable: 'crashed' }]
  const out = compare([ok, down])
  assert.equal(out.unavailableCases, 1)
  assert.equal(out.totals.detector.unavailable, 1)
  const fresh = compare([ok, down], new Map([['down', 'new case, absent at v0.5.1']]))
  assert.equal(fresh.unavailableCases, 0, 'a baseline cannot be expected to run a case new since it was built')
  assert.equal(fresh.unavailableOnChangedCases, 1)
  assert.match(renderComparison(fresh).join('\n'), /0 unavailable case\(s\) \(and 1 more on cases new or changed/)
  assert.throws(() => compare([down]), /no comparable run on any case/)
  const crashedPending = [evaluated('held', ['satisfied'], [], { pending: 'waits on a named defect' }), { unavailable: 'crashed' }]
  assert.equal(compare([ok, crashedPending]).unavailableCases, 1, 'a pending case does not excuse a crashed baseline')
  assert.throws(() => compareCorpus([], [], new Map()), /no comparable run/)
  assert.throws(() => compareCorpus([ok[0]], [], new Map()), /0 baseline result\(s\) for 1 candidate/)
})

test('only detector regressions on unchanged, non-pending cases are listed as regressed', () => {
  const lost = [evaluated('lost', ['missing']), { evaluated: evaluated('lost', ['satisfied']) }]
  const moved = [evaluated('moved', ['missing']), { evaluated: evaluated('moved', ['satisfied']) }]
  const held = [evaluated('held', ['missing'], [], { pending: 'waits on a named defect' }), { evaluated: evaluated('held', ['satisfied']) }]
  const fine = [evaluated('fine', ['satisfied']), { evaluated: evaluated('fine', ['satisfied']) }]
  const out = compare([lost, moved, held, fine], new Map([['moved', 'new case']]))
  assert.deepEqual(regressedCases(out).map((c) => c.name), ['lost'])
})

test('the text report names regressions, unavailable and label-changed cases', () => {
  const lost = [evaluated('lost', ['missing'], [unclaimedFinding(9)]), { evaluated: evaluated('lost', ['satisfied']) }]
  const down = [evaluated('down', ['held']), { unavailable: 'binary exited 2' }]
  const moved = [evaluated('moved', ['held']), { evaluated: evaluated('moved', ['held']) }]
  const text = renderComparison(compare([lost, down, moved], new Map([['moved', 'new case, absent at v0.5.1']]))).join('\n')
  assert.match(text, /2 regression\(s\), 1 unavailable case\(s\)/)
  assert.match(text, /lost: lost#1 regressed; new unclaimed duplicate-test @ a\.test\.ts:9/)
  assert.match(text, /unavailable: down -- binary exited 2/)
  assert.match(text, /label-changed: moved -- new case, absent at v0\.5\.1/)
  assert.match(text, /labels or files changed since the baseline \(1 case\(s\)/)
})

/** A fake git answering the few calls the run record makes. */
function fakeGit({ fail = false } = {}) {
  return (args) => {
    if (fail) throw new Error('git failed')
    if (args[0] === 'rev-parse') return 'abc123\n'
    throw new Error(`unexpected git ${args.join(' ')}`)
  }
}

/** A throwaway git repository with a corpus at `recall/`, committed once. */
function repo(makeCases, files = {}, { corpusAtTop = false } = {}) {
  const top = realpathSync(mkdtempSync(join(tmpdir(), 'label-repo-')))
  const cases = typeof makeCases === 'function' ? makeCases(top) : makeCases
  const run = (args) => runGit(['-c', 'user.name=t', '-c', 'user.email=t@example.com', ...args], top)
  const root = corpusAtTop ? top : join(top, 'recall')
  mkdirSync(root, { recursive: true })
  writeFileSync(join(root, 'manifest.json'), JSON.stringify({ cases }))
  const put = (name, content) => {
    mkdirSync(dirname(join(root, name)), { recursive: true })
    writeFileSync(join(root, name), content)
  }
  for (const [name, content] of Object.entries(files)) put(name, content)
  run(['init', '-q'])
  run(['add', '-A'])
  run(['commit', '-q', '-m', 'base'])
  return { top, root, run, put, changes: (manifestCases = cases, ref = 'HEAD') => readLabelChanges({ root, ref, manifestCases, git: run }) }
}

test('cases new, with other expectations, or reading other files are named with the reason', () => {
  const was = [
    { name: 'same', path: 'same', rule: 'r' },
    { name: 'relabelled', path: 'relabelled', rule: 'r' },
    { name: 'edited', path: 'edited', rule: 'r' },
  ]
  const { changes, put } = repo(was, { 'same/a.ts': '1', 'relabelled/a.ts': '1', 'edited/a.ts': '1' })
  const stamped = (c) => ({ ...c, origin: 'real', verdict: 'reviewed', classifiedAt: '2026-10-20' })
  const now = [stamped(was[0]), { ...was[1], rule: 'other' }, was[2], { name: 'fresh', path: 'fresh', rule: 'r' }]
  put('edited/a.ts', '2')
  const reasons = changes(now)
  assert.deepEqual([...reasons.keys()].sort(), ['edited', 'fresh', 'relabelled'])
  assert.match(reasons.get('fresh'), /new case, absent at HEAD/)
  assert.match(reasons.get('relabelled'), /expectations differ/)
  assert.match(reasons.get('edited'), /files or configuration differ/)
  assert.equal(reasons.has('same'), false, 'provenance labels alone are not a change')
  assert.throws(() => changes(now, 'no-such-ref'), /failed/)
})

test('what a case reads is compared whatever git would call the change', () => {
  const cases = ['a', 'b', 'c', 'd'].map((name) => ({ name, path: name }))
  const { changes, put, top } = repo(cases, { 'a/x.ts': 'a body long enough to be recognised as a rename target', 'b/y.ts': '1', 'c/z.ts': '1', 'd/w.ts': '1' })
  assert.equal(changes().size, 0)
  renameSync(join(top, 'recall', 'a', 'x.ts'), join(top, 'recall', 'b', 'x.ts'))
  put('c/new.ts', 'untracked')
  put('d/é-ü.ts', 'non-ascii name that git quotes')
  assert.deepEqual([...changes().keys()].sort(), ['a', 'b', 'c', 'd'], 'a moved, an added, an untracked and a quoted file')
})

test('the baseline tree is the whole checkout even when git runs from a subdirectory', () => {
  const cases = [{ name: 'a', path: 'a' }]
  const { top, root, put } = repo(cases, { 'a/x.ts': '1' })
  mkdirSync(join(top, 'tools'))
  const below = (args) => runGit(['-c', 'user.name=t', '-c', 'user.email=t@example.com', ...args], join(top, 'tools'))
  assert.equal(readLabelChanges({ root, ref: 'HEAD', manifestCases: cases, git: below }).size, 0)
  put('a/x.ts', '2')
  assert.deepEqual([...readLabelChanges({ root, ref: 'HEAD', manifestCases: cases, git: below }).keys()], ['a'])
})

test('a corpus outside the checkout cannot be compared with a ref of it', () => {
  const { top, run } = repo([{ name: 'a', path: 'a' }], { 'a/x.ts': '1' })
  const elsewhere = realpathSync(mkdtempSync(join(tmpdir(), 'other-corpus-')))
  assert.throws(
    () => readLabelChanges({ root: elsewhere, ref: 'HEAD', manifestCases: [], git: run }),
    new RegExp(`outside the checkout at ${top.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`),
  )
})

test('case paths are read as the runner reads them', () => {
  const cases = [{ name: 'dot', path: './a/' }, { name: 'slash', path: 'b/' }, { name: 'plain', path: 'c' }]
  const { changes, put } = repo(cases, { 'a/x.ts': '1', 'b/y.ts': '1', 'c/z.ts': '1' })
  put('a/x.ts', '2')
  put('b/y.ts', '2')
  assert.deepEqual([...changes().keys()].sort(), ['dot', 'slash'])
})

test('a case rooted at . owns every file of the corpus, whether it sits below the top or is the top', () => {
  for (const corpusAtTop of [false, true]) {
    const cases = [{ name: 'whole', path: '.' }]
    const { changes, put } = repo(cases, { 'x.test.ts': 'one' }, { corpusAtTop })
    assert.equal(changes().size, 0)
    put('x.test.ts', 'two')
    assert.deepEqual([...changes().keys()], ['whole'], String(corpusAtTop))
  }
})

test('the nearest configuration governs a case, an explicit one replaces the search, and a shadowed one does not count', () => {
  const { changes, put, top } = repo(
    (top) => [
      { name: 'nearest', path: 'sub/c' },
      { name: 'shadowed', path: 'sub/s' },
      { name: 'explicit', path: 'e', arguments: ['--config', '../shared.json'] },
      { name: 'absolute', path: 'f', arguments: ['-c', join(top, 'abs.json')] },
    ],
    { 'sub/c/x.ts': '1', 'sub/s/x.ts': '1', 'sub/s/.vitest-dedup.json': '{}', 'e/x.ts': '1', 'f/x.ts': '1', 'shared.json': '{}', '../abs.json': '{}' },
  )
  assert.equal(changes().size, 0)
  put('.vitest-dedup.json', '{"rules":{}}')
  assert.deepEqual([...changes().keys()], ['nearest'], 'only the case with no nearer file and no explicit one')
  put('../.vitest-dedup.json', '{"rules":{}}')
  assert.deepEqual([...changes().keys()], ['nearest'], 'a repository-level file, past the corpus, is shadowed the same way')
  put('shared.json', '{"rules":{}}')
  assert.deepEqual([...changes().keys()].sort(), ['explicit', 'nearest'], 'the explicit file itself changed')
  writeFileSync(join(top, 'abs.json'), '{"rules":{}}')
  assert.deepEqual([...changes().keys()].sort(), ['absolute', 'explicit', 'nearest'], 'an absolute --config inside the checkout is mapped into the baseline tree')
})

test('a case digest looks for configuration no higher than the top of its tree', () => {
  const outer = mkdtempSync(join(tmpdir(), 'digest-top-'))
  const top = join(outer, 'tree')
  mkdirSync(join(top, 'case'), { recursive: true })
  writeFileSync(join(top, 'case', 'x.ts'), '1')
  const digest = () => caseDigest({ root: top, top }, top, { name: 'c', path: 'case' })
  const before = digest()
  writeFileSync(join(outer, '.vitest-dedup.json'), '{}')
  assert.equal(digest(), before, 'a file above the tree is not part of it')
  writeFileSync(join(top, '.vitest-dedup.json'), '{}')
  assert.notEqual(digest(), before, 'one at the top is')
})

test('ignore files, absolute case paths and provenance-only manifest edits are read as the analyzer reads them', () => {
  const { changes, put, root } = repo(
    (top) => [
      { name: 'ignored', path: 'sub/ignored' },
      { name: 'sibling', path: 'other' },
      { name: 'absolute', path: join(top, 'recall', 'absolute') },
    ],
    { 'sub/ignored/a.ts': '1', 'other/b.ts': '1', 'absolute/x.ts': '1' },
  )
  const base = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8')).cases
  assert.equal(changes(base.map((c) => ({ ...c, classifiedAt: '2026-10-21' }))).size, 0, 'provenance alone changes nothing')
  put('sub/.gitignore', 'a.ts\n')
  assert.deepEqual([...changes().keys()], ['ignored'], 'an ignore file above one case changes which of its files are analyzed')
  put('absolute/x.ts', '2')
  assert.deepEqual([...changes().keys()].sort(), ['absolute', 'ignored'], 'an absolute case path is placed in each tree')
})

test('the analyzer\'s own ignore file above a case is part of what the case reads', () => {
  const { changes, put } = repo([{ name: 'c', path: 'sub/c' }, { name: 'other', path: 'other' }], { 'sub/c/a.ts': '1', 'other/b.ts': '1' })
  put('sub/.vitest-dedupignore', 'a.ts\n')
  assert.deepEqual([...changes().keys()], ['c'])
})

test('a case that owns the manifest does not change when only its provenance labels do', () => {
  const { changes, root } = repo([{ name: 'whole', path: '.' }], { 'x.ts': '1' })
  const base = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8')).cases
  writeFileSync(join(root, 'manifest.json'), JSON.stringify({ cases: base.map((c) => ({ ...c, classifiedAt: '2026-10-21' })) }))
  assert.equal(changes(base).size, 0)
})

test('a case that no longer extracts what its labels say is a regression even with no finding at stake', () => {
  const broken = { extraction: { matches: false } }
  const lost = [evaluated('e', ['satisfied'], [], broken), { evaluated: evaluated('e', ['satisfied']) }]
  const out = compare([lost])
  assert.equal(out.totals.extractionRegressed, 1)
  assert.equal(out.regressions, 1)
  assert.match(renderComparison(out).join('\n'), /e: extraction no longer matches the labels/)
  const fixed = [evaluated('e', ['satisfied']), { evaluated: evaluated('e', ['satisfied'], [], broken) }]
  assert.equal(compare([fixed]).regressions, 0)
  const both = [evaluated('e', ['satisfied'], [], broken), { evaluated: evaluated('e', ['satisfied'], [], broken) }]
  assert.equal(compare([both]).regressions, 0, 'broken in both is not news')
  const down = [evaluated('e', ['satisfied']), { unavailable: 'crashed' }]
  assert.equal(compareCase(...down.map((x, i) => (i === 0 ? x : x)), null).extraction, 'unavailable')
})

test('a file turned into a symlink, and a link into the checkout, are read as the analyzer meets them', () => {
  const { changes, top, put } = repo(
    [{ name: 'files', path: 'files' }, { name: 'linked', path: 'linked' }],
    { 'files/a.ts': 'same', 'files/b.ts': 'same', 'linked/x.ts': '1', '../settings.json': '{}' },
  )
  const root = join(top, 'recall')
  assert.equal(changes().size, 0)
  rmSync(join(root, 'files', 'b.ts'))
  symlinkSync('a.ts', join(root, 'files', 'b.ts'))
  assert.deepEqual([...changes().keys()], ['files'], 'identical bytes, but the walk skips a symlink')
  symlinkSync(join(top, 'settings.json'), join(root, 'linked', '.vitest-dedup.json'))
  assert.deepEqual([...changes().keys()].sort(), ['files', 'linked'], 'a new configuration link')
  execFileSync('git', ['add', '-A'], { cwd: top })
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '-m', 'links'], { cwd: top })
  assert.equal(changes().size, 0)
  put('../settings.json', '{"rules":{}}')
  assert.deepEqual([...changes().keys()], ['linked'], 'the target of an absolute link into the checkout is read from each tree')
})

test('a configuration outside the checkout, and an absolute link to a directory, are read the same way in both trees', () => {
  const elsewhere = realpathSync(mkdtempSync(join(tmpdir(), 'shared-config-')))
  writeFileSync(join(elsewhere, 'config.json'), '{}')
  const { top, put } = repo(
    () => [{ name: 'outside', path: 'outside', arguments: ['--config', join(elsewhere, 'config.json')] }, { name: 'dir', path: 'dir' }],
    { 'outside/x.ts': '1', 'real/y.ts': '1' },
  )
  const root = join(top, 'recall')
  symlinkSync(join(root, 'real'), join(root, 'dir'))
  execFileSync('git', ['add', '-A'], { cwd: top })
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '-m', 'link'], { cwd: top })
  const cases = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8')).cases
  const changed = () => readLabelChanges({ root, ref: 'HEAD', manifestCases: cases, git: (args) => runGit(args, top) })
  assert.equal(changed().size, 0, 'an unchanged shared configuration does not make a case look changed')
  put('real/y.ts', '2')
  assert.deepEqual([...changed().keys()], ['dir'], 'the target of an absolute directory link is read from each tree')
  writeFileSync(join(elsewhere, 'config.json'), '{"rules":{}}')
  assert.equal(changed().has('outside'), false, 'a file outside the checkout is the same file in both, so it cannot be attributed')
})

test('a case path that is a symlink is read from where the analyzer reads its configuration', () => {
  const { top } = repo([{ name: 'alias', path: 'alias' }], { 'suite/case/x.ts': '1', 'suite/.vitest-dedup.json': '{}' })
  rmSync(join(top, 'recall', 'alias'), { force: true })
  symlinkSync(join('suite', 'case'), join(top, 'recall', 'alias'))
  execFileSync('git', ['add', '-A'], { cwd: top })
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '-m', 'alias'], { cwd: top })
  const reasons = (manifestCases) => readLabelChanges({ root: join(top, 'recall'), ref: 'HEAD', manifestCases, git: (args) => runGit(args, top) })
  assert.equal(reasons([{ name: 'alias', path: 'alias' }]).size, 0)
  writeFileSync(join(top, 'recall', 'suite', '.vitest-dedup.json'), '{"rules":{}}')
  assert.deepEqual([...reasons([{ name: 'alias', path: 'alias' }]).keys()], ['alias'])
})

test('repeated unclaimed findings are counted, not collapsed', () => {
  const once = [evaluated('u', ['satisfied'], [unclaimedFinding(5)]), { evaluated: evaluated('u', ['satisfied'], [unclaimedFinding(5)]) }]
  assert.equal(compare([once]).regressions, 0)
  const twice = [evaluated('u', ['satisfied'], [unclaimedFinding(5), unclaimedFinding(5)]), { evaluated: evaluated('u', ['satisfied'], [unclaimedFinding(5)]) }]
  assert.equal(compare([twice]).totals.newUnclaimed, 1)
  assert.equal(compare([twice]).regressions, 1)
  const fewer = [evaluated('u', ['satisfied'], [unclaimedFinding(5)]), { evaluated: evaluated('u', ['satisfied'], [unclaimedFinding(5), unclaimedFinding(5)]) }]
  assert.equal(compare([fewer]).totals.goneUnclaimed, 1)
})

test('matcher lists are sets and key order is nothing', () => {
  const matcher = { rule: 'r', titles: ['x', 'y'], locations: ['p:1', 'p:2'] }
  const other = { rule: 'r', titles: ['z', 'w'] }
  const cases = [{ name: 'a', path: 'a', expectedFindings: [matcher, other] }]
  const { changes } = repo(cases, { 'a/x.ts': '1' })
  const flipped = [{ expectedFindings: [{ titles: ['w', 'z'], rule: 'r' }, { locations: ['p:2', 'p:1'], titles: ['y', 'x'], rule: 'r' }], path: 'a', name: 'a' }]
  assert.equal(changes(flipped).size, 0)
  assert.equal(changes([{ ...cases[0], expectedFindings: [matcher] }]).size, 1, 'a dropped matcher is a change')
})

test('the corpus digest follows names and contents', () => {
  const root = mkdtempSync(join(tmpdir(), 'corpus-digest-'))
  mkdirSync(join(root, 'a'))
  writeFileSync(join(root, 'a', 'x.ts'), 'one')
  const first = corpusDigest(root)
  assert.equal(corpusDigest(root), first)
  writeFileSync(join(root, 'a', 'x.ts'), 'two')
  assert.notEqual(corpusDigest(root), first, 'content')
  writeFileSync(join(root, 'a', 'x.ts'), 'one')
  assert.equal(corpusDigest(root), first)
  writeFileSync(join(root, 'a', 'y.ts'), 'one')
  assert.notEqual(corpusDigest(root), first, 'a new file')
})

test('a dangling link in the corpus and a binary named on PATH do not stop the comparison', () => {
  const dir = mkdtempSync(join(tmpdir(), 'run-record-link-'))
  writeFileSync(join(dir, 'x.ts'), '1')
  const before = corpusDigest(dir)
  symlinkSync('missing-target', join(dir, 'dangling'))
  const withLink = corpusDigest(dir)
  assert.notEqual(withLink, before, 'the link is part of the corpus')
  const record = buildRunRecord({ candidate: 'vitest-dedup', baseline: 'vitest-dedup-old', root: dir, ref: null, cases: [], git: fakeGit(), version: () => 'v' })
  assert.equal(record.candidate.sha256, null, 'a name found on PATH has no file to digest here')
  assert.equal(record.corpus.sha256, withLink)
})

test('the run record names both binaries, the corpus, the commits and the case arguments', () => {
  const dir = mkdtempSync(join(tmpdir(), 'run-record-'))
  writeFileSync(join(dir, 'new'), 'candidate')
  writeFileSync(join(dir, 'old'), 'baseline')
  const cases = [{ name: 'a', arguments: ['--rule', 'subsumed-test=warning'] }, { name: 'b', arguments: [] }]
  const record = buildRunRecord({
    candidate: join(dir, 'new'),
    baseline: join(dir, 'old'),
    root: dir,
    ref: 'v0.5.1',
    cases,
    git: fakeGit(),
    version: (binary) => `v for ${binary.split('/').pop()}`,
  })
  assert.notEqual(record.candidate.sha256, record.baseline.sha256)
  assert.equal(record.candidate.version, 'v for new')
  assert.equal(record.baseline.commit, 'abc123')
  assert.equal(record.checkoutCommit, 'abc123')
  assert.match(record.corpus.sha256, /^[0-9a-f]{64}$/)
  assert.deepEqual(record.caseArguments, { a: ['--rule', 'subsumed-test=warning'] })
  const unresolved = buildRunRecord({ candidate: join(dir, 'new'), baseline: join(dir, 'old'), root: dir, ref: null, cases, git: fakeGit({ fail: true }), version: () => 'v' })
  assert.equal(unresolved.baseline.commit, null)
  assert.equal(unresolved.checkoutCommit, null, 'an unavailable commit is recorded as null, not guessed')
})

test('binary versions and git calls report failure rather than guessing', () => {
  assert.match(binaryVersion(process.execPath), /^v\d+/)
  assert.equal(binaryVersion('/nonexistent/analyzer'), 'unavailable')
  assert.match(runGit(['--version'], tmpdir()), /git version/)
  assert.throws(() => runGit(['definitely-not-a-command'], tmpdir()), /failed/)
  assert.throws(() => runGit(['--version'], '/nonexistent/dir'))
})
