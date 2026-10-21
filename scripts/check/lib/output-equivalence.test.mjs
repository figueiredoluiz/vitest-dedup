import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  existsSync,
  linkSync,
  lstatSync,
  chmodSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
  mkdirSync,
} from 'node:fs'
import { spawn, spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { dirname, join, relative, resolve, sep } from 'node:path'
import {
  changedFiles,
  compareRuns,
  DELETED,
  firstDifference,
  missingAncestor,
  normalize,
  outputArgument,
  pairRuns,
  readRecords,
  restore,
  snapshot,
  summarize,
  versionReplacements,
  writeLocations,
} from './output-equivalence.mjs'

/** Analyzer run record with defaults for every field `over` does not set. */
const run = (over = {}) => ({ status: 0, signal: null, stdout: '', stderr: '', files: null, ...over })

/** Both `--name VALUE` and `--name=VALUE` are found; a trailing flag without a value is not. */
test('option values are found in both spellings', () => {
  const spaced = ['.', '--reporters', 'json', '--output', 'report', '--no-metrics']
  assert.deepEqual(outputArgument(spaced), { index: 3, inline: false, value: 'report' })
  assert.deepEqual(outputArgument(['.', '--output=report']), { index: 1, inline: true, value: 'report' })
  assert.equal(outputArgument(['.', '--reporters', 'terminal']), null)
  assert.equal(outputArgument(['.', '--output']), null)
  assert.deepEqual(outputArgument(['.', '--baseline-from-ref', 'HEAD', '--baseline=b.json'], '--baseline'), {
    index: 3,
    inline: true,
    value: 'b.json',
  })
  assert.equal(outputArgument(['.', '--baseline-from-ref', 'HEAD'], '--baseline'), null)
})

/** One file per file reporter, named from `--output`; only `--update-baseline` writes `--baseline`. */
test('write locations follow the reporters, the output stem and the baseline flags', () => {
  const cwd = resolve('/project')
  const at = (path) => resolve('/project', path)
  assert.deepEqual(writeLocations(['.', '--reporters', 'terminal'], cwd), [])
  assert.deepEqual(writeLocations(['.', '--reporters', 'jsonl'], cwd), [])
  assert.deepEqual(writeLocations(['.'], cwd), [])
  assert.deepEqual(writeLocations(['.', '--reporters', 'json'], cwd), [at('reports/vitest-dedup.json')])
  assert.deepEqual(writeLocations(['.', '--reporters=json,html,sarif', '--output', 'out/r'], cwd), [
    at('out/r.json'),
    at('out/r.html'),
    at('out/r.sarif'),
  ])
  assert.deepEqual(writeLocations(['.', '--reporters', 'jsonl,json', '--output=r'], cwd), [at('r.json')])
  assert.deepEqual(writeLocations(['.', '--reporters', 'json', '--baseline', 'b.json'], cwd), [
    at('reports/vitest-dedup.json'),
  ])
  assert.deepEqual(writeLocations(['.', '--reporters', 'json', '--baseline', 'b.json', '--update-baseline'], cwd), [
    at('reports/vitest-dedup.json'),
    at('b.json'),
  ])
  assert.deepEqual(writeLocations(['.', '--update-baseline', '--baseline=b.json'], cwd), [at('b.json')])
  assert.deepEqual(writeLocations(['.', '--update-baseline'], cwd), [])
  assert.deepEqual(writeLocations(['--version'], cwd), [])
  // Words after `--` are paths, not options.
  assert.deepEqual(writeLocations(['--', '--reporters', 'json'], cwd), [])
})

/** The analyzer appends the extension to the stem's last component, whatever the stem looks like. */
test("report paths follow the analyzer's stem rule", () => {
  const cwd = resolve('/project')
  const json = (output) => writeLocations(['.', '--reporters', 'json', '--output', output], cwd)[0]
  assert.equal(json('a/b.c'), resolve('/project/a/b.c.json'))
  assert.equal(json('a/b/'), resolve('/project/a/b.json'))
  assert.equal(json('a/b/.'), resolve('/project/a/b.json'))
  // No last component to extend: the analyzer puts its default stem inside the directory.
  assert.equal(json('.'), resolve('/project/vitest-dedup.json'))
  assert.equal(json('..'), resolve('/vitest-dedup.json'))
  assert.equal(json('../r'), resolve('/r.json'))
  assert.equal(json('/abs/r'), resolve('/abs/r.json'))
})

/** The analyzer leaves `..` to the filesystem, which follows a link in front of it; this check cannot. */
test('a .. after a named component in a write location is refused', () => {
  const cwd = resolve('/project')
  for (const output of ['a/../b', 'a/..', '/abs/x/../y']) {
    assert.throws(
      () => writeLocations(['.', '--reporters', 'json', '--output', output], cwd),
      /unsupported write location/,
      output,
    )
  }
  assert.throws(
    () => writeLocations(['.', '--update-baseline', '--baseline', 'dir/../b.json'], cwd),
    /unsupported write location/,
  )
  // Nothing is written without a file reporter, so the same spelling is not a problem there.
  assert.deepEqual(writeLocations(['.', '--reporters', 'terminal', '--output', 'a/../b'], cwd), [])
})

/** `--output` and `--baseline` are global: whichever side of `check` comes last wins, so every one is watched. */
test('global options given before and after check are all watched', () => {
  const cwd = resolve('/project')
  const at = (path) => resolve('/project', path)
  const args = ['--output', 'a', 'check', 'p', '--output', 'b', '--reporters', 'json']
  assert.deepEqual(writeLocations(args, cwd), [at('a.json'), at('b.json')])
  const update = ['--baseline', 'x.json', 'check', '--baseline=y.json', '--update-baseline']
  assert.deepEqual(writeLocations(update, cwd), [at('x.json'), at('y.json')])
})

/** Clap appends repeated `--reporters`, so a later occurrence still selects a file report. */
test('repeated --reporters all select reports', () => {
  const cwd = resolve('/project')
  const at = (path) => resolve('/project', path)
  assert.deepEqual(writeLocations(['.', '--reporters', 'terminal', '--reporters', 'json', '--output', 'r'], cwd), [
    at('r.json'),
  ])
  assert.deepEqual(writeLocations(['.', '--reporters=json', '--reporters', 'html,sarif', '--output=r'], cwd), [
    at('r.json'),
    at('r.html'),
    at('r.sarif'),
  ])
})

/** The version is rewritten only where the analyzer prints it, never in a title or path that contains it. */
test('version replacements cover the printed contexts only', () => {
  const replacements = versionReplacements(['0.7.0', '0.6.0'])
  const rewrite = (text, scope = 'stdout') => normalize(text, replacements, scope)
  assert.equal(rewrite('VitestDedup v0.7.0'), 'VitestDedup v0.6.0')
  assert.equal(rewrite('vitest-dedup 0.7.0'), 'vitest-dedup 0.6.0')
  assert.equal(rewrite('  "semanticVersion": "0.7.0",', 'sarif'), '  "semanticVersion": "0.6.0",')
  assert.equal(rewrite('"semanticVersion": "0.7.0"', 'sarif'), '"semanticVersion": "0.6.0"')
  // A path or title that spells the field is content.
  assert.equal(rewrite('missing /x/"semanticVersion": "0.7.0"', 'sarif'), 'missing /x/"semanticVersion": "0.7.0"')
  // Each rule applies only to the output the version is printed in: an HTML report embedding a test
  // source, or stderr, that has the same line is content.
  assert.equal(rewrite('VitestDedup v0.7.0', 'file'), 'VitestDedup v0.7.0')
  assert.equal(rewrite('VitestDedup v0.7.0', 'stderr'), 'VitestDedup v0.7.0')
  assert.equal(rewrite('vitest-dedup 0.7.0', 'sarif'), 'vitest-dedup 0.7.0')
  assert.equal(rewrite('  "semanticVersion": "0.7.0",', 'stdout'), '  "semanticVersion": "0.7.0",')
  assert.equal(rewrite('  "semanticVersion": "0.7.0",', 'file'), '  "semanticVersion": "0.7.0",')
  assert.equal(rewrite('{"titles":["accepts 0.7.0"]}'), '{"titles":["accepts 0.7.0"]}')
  assert.equal(rewrite('path/to/0.7.0/file'), 'path/to/0.7.0/file')
  // A title that contains a printed context is still content.
  assert.equal(rewrite('{"titles":["accepts VitestDedup v0.7.0"]}'), '{"titles":["accepts VitestDedup v0.7.0"]}')
  assert.equal(rewrite('  VitestDedup v0.7.0'), '  VitestDedup v0.7.0')
  assert.equal(rewrite('accepts vitest-dedup 0.7.0'), 'accepts vitest-dedup 0.7.0')
  assert.equal(rewrite('x\nVitestDedup v0.7.0\ny'), 'x\nVitestDedup v0.6.0\ny')
  assert.equal(rewrite('VitestDedup v0.7.01'), 'VitestDedup v0.7.01')
})

/** Timing values and replacements are erased; every other number survives, so counts still compare. */
test('normalization erases timing, peak RSS and replacements but keeps other numbers', () => {
  assert.equal(normalize('Found 2 findings.\nDetection time: 41.7 ms'), 'Found 2 findings.\nDetection time: 0 ms')
  assert.equal(normalize('Detection time: 3 ms'), 'Detection time: 0 ms')
  // Timing and peak RSS are stdout facts; the same lines in a file or on stderr are content.
  for (const scope of ['file', 'stderr', 'sarif']) {
    assert.notEqual(normalize('Detection time: 1 ms', [], scope), normalize('Detection time: 2 ms', [], scope))
    assert.notEqual(
      normalize('{"type":"memory","peakRssBytes":1}', [], scope),
      normalize('{"type":"memory","peakRssBytes":2}', [], scope),
    )
  }
  const memory = '{"type":"memory","retainedBytes":2269,"peakRssBytes":6782976,"bytesPerTest":1134}'
  assert.equal(normalize(memory), '{"type":"memory","retainedBytes":2269,"peakRssBytes":0,"bytesPerTest":1134}')
  // Only a `memory` record carries the process figure; the same text elsewhere is content.
  assert.notEqual(normalize('{"peakRssBytes":1}'), normalize('{"peakRssBytes":2}'))
  assert.notEqual(
    normalize('    --> a.test.ts:1:1  "peakRssBytes":1'),
    normalize('    --> a.test.ts:1:1  "peakRssBytes":2'),
  )
  assert.notEqual(
    normalize('{"type":"finding","titles":["peakRssBytes\\":1"]}'),
    normalize('{"type":"finding","titles":["peakRssBytes\\":2"]}'),
  )
  assert.equal(
    normalize('x\n{"type":"memory","a":1,"peakRssBytes":5,"b":2}\ny'),
    'x\n{"type":"memory","a":1,"peakRssBytes":0,"b":2}\ny',
  )
  assert.equal(
    normalize('/tmp/a/vitest-dedup.json 0.7.0', [
      ['/tmp/a', '<output>'],
      ['0.7.0', '0.6.0'],
    ]),
    '<output>/vitest-dedup.json 0.6.0',
  )
  // Only the footer line is timing: the same words inside content are compared as they are.
  assert.notEqual(normalize('{"titles":["Detection time: 1 ms"]}'), normalize('{"titles":["Detection time: 2 ms"]}'))
  assert.notEqual(normalize('  Detection time: 1 ms'), normalize('  Detection time: 2 ms'))
  assert.equal(normalize('a\nDetection time: 1 ms\nb'), 'a\nDetection time: 0 ms\nb')
  // A replacement never rewrites part of a longer number.
  assert.equal(
    normalize('v10.7.0 v0.7.0 v0.7.01 0.7.0.1 0.7.0', [['0.7.0', '0.6.0']]),
    'v10.7.0 v0.6.0 v0.7.01 0.7.0.1 0.6.0',
  )
  assert.equal(normalize('a $& b', [['a', '$&']]), '$& $& b')
  // Only the two named fields are erased: a changed count or retained figure is a difference.
  assert.notEqual(normalize('Analyzed 369 tests'), normalize('Analyzed 370 tests'))
  assert.notEqual(normalize('"retainedBytes":1'), normalize('"retainedBytes":2'))
  assert.notEqual(normalize('Detection time: fast'), normalize('Detection time: slow'))
})

/** The first differing line is reported with both sides, including a missing trailing line. */
test('first difference names the line and both sides', () => {
  assert.equal(firstDifference('a\nb', 'a\nb'), null)
  assert.deepEqual(firstDifference('a\nb', 'a\nc'), { line: 2, baseline: 'b', candidate: 'c' })
  assert.deepEqual(firstDifference('a', 'a\nb'), { line: 2, baseline: '<end>', candidate: 'b' })
})

/** Identical runs compare equal; an exit, stream or file change is each reported as its own part. */
test('runs differing in exit, stream or file content are reported per part', () => {
  const files = new Map([['vitest-dedup.json', '{"findings":1,"peakRssBytes":3}']])
  assert.deepEqual(compareRuns(run({ files }), run({ files: new Map(files) })), [])
  // Files are compared exactly: no stdout normalization reaches them.
  const changed = new Map([['vitest-dedup.json', '{"findings":1,"peakRssBytes":9}']])
  assert.deepEqual(
    compareRuns(run({ files }), run({ files: changed })).map((d) => d.part),
    ['file:vitest-dedup.json'],
  )
  assert.deepEqual(
    compareRuns(run({ stdout: 'Detection time: 1 ms\n' }), run({ stdout: 'Detection time: 9 ms\n' })),
    [],
  )
  assert.deepEqual(compareRuns(run(), run({ status: 1 })), [{ part: 'exit', baseline: 0, candidate: 1 }])
  assert.deepEqual(compareRuns(run(), run({ signal: 'SIGKILL', status: null })), [
    { part: 'exit', baseline: 0, candidate: 'SIGKILL' },
  ])
  assert.deepEqual(
    compareRuns(run({ stderr: 'warn' }), run()).map((d) => d.part),
    ['stderr'],
  )
})

/** A report written by only one side is a difference, not a silent skip. */
test('a file present on one side only is a difference', () => {
  const files = new Map([['vitest-dedup.json', '{}']])
  assert.deepEqual(compareRuns(run({ files }), run({ files: new Map() })), [
    { part: 'file:vitest-dedup.json', baseline: 'present', candidate: 'missing' },
  ])
})

/** Each side's own replacements map its output directory to one token, so differing paths agree. */
test('side-specific replacements reconcile output directories', () => {
  const baseline = run({ stdout: 'wrote /tmp/scratch/vitest-dedup.json' })
  const candidate = run({ stdout: 'wrote /work/report/vitest-dedup.json' })
  assert.equal(compareRuns(baseline, candidate).length, 1)
  assert.deepEqual(
    compareRuns(baseline, candidate, {
      baseline: [['/tmp/scratch', '<output>']],
      candidate: [['/work/report', '<output>']],
    }),
    [],
  )
})

/** A suite with no invocation fails the verdict even when nothing mismatched. */
test('summary fails on mismatches and on suites that made no invocation', () => {
  const clean = { suite: 'corpus', differences: [] }
  const dirty = { suite: 'recall', differences: [{ part: 'exit' }] }
  assert.equal(summarize([clean], ['corpus']).ok, true)
  const empty = summarize([clean], ['corpus', 'recall'])
  assert.deepEqual([empty.ok, empty.empty], [false, ['recall']])
  const mismatch = summarize([clean, dirty], ['corpus', 'recall'])
  assert.deepEqual(
    [mismatch.ok, mismatch.mismatches.length, mismatch.invocations],
    [false, 1, { corpus: 1, recall: 1 }],
  )
})

/** Output that is not valid UTF-8 compares by its exact bytes; valid text is still normalized. */
test('binary output compares by bytes', () => {
  /** A run whose stdout and report are `bytes`. */
  const out = (bytes) => run({ stdout: Buffer.from(bytes), files: new Map([['report.bin', Buffer.from(bytes)]]) })
  assert.deepEqual(
    compareRuns(out([0xff]), out([0xfe])).map(({ part }) => part),
    ['stdout', 'file:report.bin'],
  )
  assert.deepEqual(compareRuns(out([0xff]), out([0xff])), [])
  assert.deepEqual(
    compareRuns(out([0xff]), out([0x78])).map(({ part }) => part),
    ['stdout', 'file:report.bin'],
  )
  assert.deepEqual(
    compareRuns(
      run({ stdout: Buffer.from('Detection time: 3 ms') }),
      run({ stdout: Buffer.from('Detection time: 9 ms') }),
    ),
    [],
  )
})

/** A byte-order mark is content: output that gains or loses one differs. */
test('a byte-order mark is not dropped', () => {
  /** A run whose report is `bytes`. */
  const report = (bytes) => run({ files: new Map([['vitest-dedup.json', Buffer.from(bytes)]]) })
  const bom = [0xef, 0xbb, 0xbf, 0x7b, 0x7d]
  assert.deepEqual(
    compareRuns(report(bom), report([0x7b, 0x7d])).map(({ part }) => part),
    ['file:vitest-dedup.json'],
  )
  assert.deepEqual(compareRuns(report(bom), report(bom)), [])
})

/** Runs `check` with a fresh temporary directory, removed afterwards. */
function withTemporary(check) {
  const root = mkdtempSync(join(tmpdir(), 'equivalence-test-'))
  try {
    check(root)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

/** Paths and values of `changedFiles`, sorted by path, with each file's bytes as text. */
const changes = (location, before) =>
  changedFiles(location, before)
    .map(([path, value]) => [path, Buffer.isBuffer(value) ? value.toString() : value])
    .sort(([left], [right]) => left.localeCompare(right))

/** A missing location snapshots as empty; a file location records one file; a directory records its tree. */
test('snapshot records what exists and nothing for a missing location', () =>
  withTemporary((root) => {
    assert.equal(snapshot(join(root, 'absent', 'reports')).size, 0)
    writeFileSync(join(root, 'baseline.json'), '[]')
    assert.deepEqual(
      [...snapshot(join(root, 'baseline.json')).values()].map((entry) => entry.kind),
      ['file'],
    )
    mkdirSync(join(root, 'reports', 'nested'), { recursive: true })
    writeFileSync(join(root, 'reports', 'nested', 'a.json'), '{}')
    assert.deepEqual(
      [...snapshot(join(root, 'reports')).keys()],
      [join(root, 'reports'), join(root, 'reports', 'nested'), join(root, 'reports', 'nested', 'a.json')],
    )
  }))

/** Links, the location itself included, are leaves recorded by target: a loop or a link outside is never followed. */
test('snapshot treats symbolic links as leaves', { skip: process.platform === 'win32' }, () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    const outside = join(root, 'outside')
    mkdirSync(reports)
    mkdirSync(outside)
    writeFileSync(join(outside, 'secret.json'), '{}')
    symlinkSync(reports, join(reports, 'loop'))
    symlinkSync(outside, join(reports, 'out'))
    assert.deepEqual(
      [...snapshot(reports).keys()].sort(),
      [reports, join(reports, 'loop'), join(reports, 'out')].sort(),
    )
    assert.deepEqual(snapshot(join(reports, 'out')).get(join(reports, 'out')), { kind: 'link', target: outside })
    // A trailing separator still names the link, not the directory it points to.
    assert.deepEqual(
      [...snapshot(`${join(reports, 'out')}/`)],
      [[join(reports, 'out'), { kind: 'link', target: outside }]],
    )
  }),
)

/**
 * Every spelling of a location records the same entries under the same absolute keys, and a snapshot
 * taken through one spelling and compared through another reports no change: path spelling must never
 * invent or hide output. Links at or below the resolved location are leaves recorded by target; a link
 * above it is part of the configured path, traversed and kept as spelled in the keys.
 */
test('snapshot keys do not depend on how a location is spelled', () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    mkdirSync(join(reports, 'nested'), { recursive: true })
    writeFileSync(join(reports, 'vitest-dedup.json'), '{}')
    writeFileSync(join(reports, 'nested', 'a.json'), '{}')
    writeFileSync(join(root, 'baseline.json'), '[]')
    const tree = [
      reports,
      join(reports, 'vitest-dedup.json'),
      join(reports, 'nested'),
      join(reports, 'nested', 'a.json'),
    ]
    const links = process.platform !== 'win32'
    if (links) {
      mkdirSync(join(root, 'outside'))
      writeFileSync(join(root, 'outside', 'secret.json'), '{}')
      symlinkSync(join(root, 'outside'), join(root, 'dir-link'))
      symlinkSync(join(root, 'baseline.json'), join(root, 'file-link'))
      symlinkSync(join(root, 'missing'), join(root, 'dangling'))
      symlinkSync(root, join(root, 'parent-link'))
    }
    const viaParent = links ? join(root, 'parent-link', 'reports') : null
    // [spelling, expected entries as [key, kind]]
    const rows = [
      [reports, tree.map((key) => [key, key.endsWith('.json') ? 'file' : 'directory'])],
      [`${reports}${sep}`, null],
      [`${reports}${sep}${sep}`, null],
      // Interpolated, not `join`ed: `join` would normalize the dot segments away before `snapshot` sees them.
      [`${root}${sep}x${sep}..${sep}reports`, null],
      [relative(process.cwd(), reports) || '.', null],
      [join(root, 'baseline.json'), [[join(root, 'baseline.json'), 'file']]],
      [`${join(root, 'baseline.json')}${sep}`, [[join(root, 'baseline.json'), 'file']]],
      [join(root, 'absent'), []],
      [`${join(root, 'absent', 'deeper')}${sep}`, []],
      ...(links
        ? [
            [join(root, 'dir-link'), [[join(root, 'dir-link'), 'link']]],
            [`${join(root, 'dir-link')}${sep}`, [[join(root, 'dir-link'), 'link']]],
            [join(root, 'file-link'), [[join(root, 'file-link'), 'link']]],
            [join(root, 'dangling'), [[join(root, 'dangling'), 'link']]],
            // A link above the location is the configured path, not output: walked, keyed as spelled.
            [
              viaParent,
              tree.map((key) => [key.replace(reports, viaParent), key.endsWith('.json') ? 'file' : 'directory']),
            ],
          ]
        : []),
    ]
    const expectedTree = rows[0][1]
    for (const [spelling, expected] of rows) {
      const entries = [...snapshot(spelling)]
        .map(([key, entry]) => [key, entry.kind])
        .sort(([a], [b]) => a.localeCompare(b))
      assert.deepEqual(
        entries,
        [...(expected ?? expectedTree)].sort(([a], [b]) => a.localeCompare(b)),
        spelling,
      )
    }
    // A snapshot through one spelling, compared through another, shows no change.
    for (const [first] of rows.slice(0, 5)) {
      for (const [second] of rows.slice(0, 5))
        assert.deepEqual(changedFiles(second, snapshot(first)), [], `${first} then ${second}`)
    }
  }))

/** A location under a file (ENOTDIR) is absent, not an error, so a parent replaced by a file reports deletions. */
test('a location under a file snapshots as empty', () =>
  withTemporary((root) => {
    writeFileSync(join(root, 'file.json'), '{}')
    assert.equal(snapshot(join(root, 'file.json', 'not-a-location')).size, 0)
    const reports = join(root, 'reports')
    const nested = join(reports, 'nested')
    mkdirSync(nested, { recursive: true })
    writeFileSync(join(nested, 'a.json'), '{}')
    const before = snapshot(nested)
    rmSync(reports, { recursive: true })
    writeFileSync(reports, 'now a file')
    assert.deepEqual(changedFiles(nested, before), [[join(nested, 'a.json'), DELETED]])
  }))

/** A FIFO, inside a location or as the location itself, fails instead of blocking or passing silently. */
test('snapshot rejects unsupported file types', { skip: process.platform === 'win32' }, () =>
  withTemporary((root) => {
    assert.equal(spawnSync('mkfifo', [join(root, 'pipe')]).status, 0)
    assert.throws(() => snapshot(root), /unsupported file type in a write location/)
    assert.throws(() => snapshot(join(root, 'pipe')), /unsupported file type in a write location/)
  }),
)

/** Created, rewritten (by time or, within one tick, by bytes) and deleted files are changes; untouched ones are not. */
test('changed files are what a run created, rewrote or deleted', () =>
  withTemporary((root) => {
    const kept = join(root, 'kept.json')
    const same = join(root, 'same-bytes.json')
    const tick = join(root, 'same-tick.json')
    const gone = join(root, 'gone.json')
    const replaced = join(root, 'replaced.json')
    for (const path of [kept, same, tick, gone, replaced]) {
      writeFileSync(path, '{}')
      utimesSync(path, 1, 1)
    }
    const before = snapshot(root)
    writeFileSync(same, '{}')
    writeFileSync(tick, '{"a":1}')
    utimesSync(tick, 1, 1)
    rmSync(gone)
    rmSync(replaced)
    mkdirSync(replaced)
    writeFileSync(join(root, 'created.json'), 'new')
    assert.deepEqual(changes(root, before), [
      [join(root, 'created.json'), 'new'],
      [gone, DELETED],
      [replaced, DELETED],
      [same, '{}'],
      [tick, '{"a":1}'],
    ])
    assert.deepEqual(changes(root, snapshot(root)), [])
  }))

/** A deletion, a link and file content never compare equal to each other, even content spelling a marker. */
test('deleted files and links have their own states', () => {
  /** A run whose only file is `value`. */
  const file = (value) => run({ files: new Map([['report', value]]) })
  assert.deepEqual(compareRuns(file(DELETED), file(Buffer.from('<deleted>'))), [
    { part: 'file:report', baseline: 'deleted', candidate: 'present' },
  ])
  assert.deepEqual(compareRuns(file(DELETED), file('<deleted>')), [
    { part: 'file:report', baseline: 'deleted', candidate: 'present' },
  ])
  assert.deepEqual(compareRuns(file({ link: 'a' }), file({ link: 'b' })), [
    { part: 'file:report', baseline: 'link -> a', candidate: 'link -> b' },
  ])
  assert.deepEqual(compareRuns(file(DELETED), file(DELETED)), [])
  assert.deepEqual(compareRuns(file({ link: 'a' }), file({ link: 'a' })), [])
  // Each side's path replacements apply to link targets, as to file contents.
  const sides = { baseline: [['/tmp/scratch', '<output>']], candidate: [['/work/report', '<output>']] }
  assert.deepEqual(compareRuns(file({ link: '/tmp/scratch/r.json' }), file({ link: '/work/report/r.json' }), sides), [])
  assert.deepEqual(
    compareRuns(file({ link: '/tmp/scratch/r.json' }), file({ link: '/work/report/s.json' }), sides).map(
      ({ part }) => part,
    ),
    ['file:report'],
  )
})

/** A tree's contents as `[relative path, kind, file text or link target]`, for comparing whole states. */
const describe = (root) =>
  [...snapshot(root)]
    .map(([path, entry]) => [
      relative(root, path),
      entry.kind,
      entry.kind === 'file' ? entry.bytes.toString() : (entry.target ?? ''),
    ])
    .sort(([left], [right]) => left.localeCompare(right))

/** Every kind of change a run can make is undone by `restore`, and nothing outside the location changes. */
test('restore returns a location to its snapshot', () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    const outside = join(root, 'outside.json')
    const links = process.platform !== 'win32'
    /** Builds the same starting tree in `reports`. */
    const seed = () => {
      rmSync(reports, { recursive: true, force: true })
      mkdirSync(join(reports, 'dir'), { recursive: true })
      for (const name of ['kept.json', 'gone.json', 'to-dir.json']) writeFileSync(join(reports, name), name)
      writeFileSync(join(reports, 'dir', 'inner.json'), 'inner')
      if (links) symlinkSync('kept.json', join(reports, 'link'))
      writeFileSync(outside, 'outside')
    }
    // [name, change a run makes]
    const rows = [
      ['rewrite', () => writeFileSync(join(reports, 'kept.json'), 'changed')],
      [
        'same tick',
        () => {
          writeFileSync(join(reports, 'kept.json'), 'kept.jsoX')
          utimesSync(join(reports, 'kept.json'), 1, 1)
        },
      ],
      [
        'create',
        () => {
          mkdirSync(join(reports, 'new', 'deep'), { recursive: true })
          writeFileSync(join(reports, 'new', 'deep', 'x.json'), 'x')
        },
      ],
      ['delete', () => rmSync(join(reports, 'gone.json'))],
      [
        'file to directory',
        () => {
          rmSync(join(reports, 'to-dir.json'))
          mkdirSync(join(reports, 'to-dir.json'))
        },
      ],
      [
        'directory to file',
        () => {
          rmSync(join(reports, 'dir'), { recursive: true })
          writeFileSync(join(reports, 'dir'), 'file')
        },
      ],
      [
        'hard link to an outside file',
        () => {
          rmSync(join(reports, 'kept.json'))
          linkSync(outside, join(reports, 'kept.json'))
        },
      ],
      ...(links
        ? [
            ['create link', () => symlinkSync(outside, join(reports, 'new-link'))],
            [
              'retarget link',
              () => {
                rmSync(join(reports, 'link'))
                symlinkSync('gone.json', join(reports, 'link'))
              },
            ],
            ['delete link', () => rmSync(join(reports, 'link'))],
            [
              'location replaced by a link',
              () => {
                rmSync(reports, { recursive: true })
                symlinkSync(root, reports)
              },
            ],
          ]
        : []),
    ]
    for (const [name, change] of rows) {
      seed()
      const expected = describe(reports)
      const before = snapshot(reports)
      change()
      restore(reports, before)
      assert.deepEqual(describe(reports), expected, name)
      assert.equal(readFileSync(outside, 'utf8'), 'outside', name)
      assert.ok(lstatSync(reports).isDirectory(), name)
    }
  }))

/** A file restore recreates keeps its permission bits, so restoring never widens access to a private file. */
test("restore keeps a file's permissions", { skip: process.platform === 'win32' }, () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    const secret = join(reports, 'secret.json')
    mkdirSync(reports)
    writeFileSync(secret, 'private')
    chmodSync(secret, 0o600)
    const before = snapshot(reports)
    writeFileSync(secret, 'changed')
    chmodSync(secret, 0o644)
    restore(reports, before)
    // One read of the restored file: its mode and bytes from the same descriptor.
    const restored = snapshot(secret).get(secret)
    assert.equal(restored.mode & 0o777, 0o600)
    assert.equal(restored.bytes.toString(), 'private')
  }),
)

/** A location inside another through a symbolic link is rejected before either run: the outer snapshot stops at the link. */
test(
  'paired runs reject a location reached through a link inside another',
  { skip: process.platform === 'win32' },
  () =>
    withTemporary((root) => {
      const reports = join(root, 'reports')
      mkdirSync(reports)
      mkdirSync(join(root, 'elsewhere'))
      symlinkSync(join(root, 'elsewhere'), join(reports, 'linked'))
      let ran = false
      /** Records that a run started. */
      const mark = () => {
        ran = true
        return run()
      }
      assert.throws(
        () => pairRuns([reports, join(reports, 'linked', 'baseline.json')], mark, mark),
        /unsupported write location .* through the symbolic link/,
      )
      // The outer location itself a link, with the nested one its direct child.
      assert.throws(
        () => pairRuns([join(reports, 'linked'), join(reports, 'linked', 'a.json')], mark, mark),
        /through the symbolic link/,
      )
      assert.equal(ran, false)
      // Control: the same nesting through a real directory is accepted.
      mkdirSync(join(reports, 'real'))
      assert.deepEqual(
        pairRuns([reports, join(reports, 'real', 'baseline.json')], mark, mark).map(({ files }) => files.size),
        [0, 0],
      )
    }),
)

/** Restoring a location that was missing removes every directory a run created on the way to it. */
test('restore removes the missing ancestors a run created', () =>
  withTemporary((root) => {
    const location = join(root, 'fresh', 'deep', 'baseline.json')
    const ancestor = missingAncestor(location)
    assert.equal(ancestor, join(root, 'fresh'))
    assert.equal(missingAncestor(root), null)
    // A dangling link exists as a link; a path under a file does not exist.
    if (process.platform !== 'win32') {
      symlinkSync(join(root, 'nowhere'), join(root, 'dangling'))
      assert.equal(missingAncestor(join(root, 'dangling')), null)
    }
    writeFileSync(join(root, 'file.json'), '{}')
    assert.equal(missingAncestor(join(root, 'file.json', 'child')), join(root, 'file.json', 'child'))
    const before = snapshot(ancestor)
    mkdirSync(dirname(location), { recursive: true })
    writeFileSync(location, '[]')
    restore(ancestor, before)
    assert.equal(existsSync(join(root, 'fresh')), false)
  }))

/**
 * Paired runs: a pre-existing file is not output, each run's changes inside the locations are, the
 * candidate starts from the pre-baseline state, and how a location is spelled changes nothing.
 */
test("paired runs compare each run's changes from one starting state", () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    const report = join(reports, 'vitest-dedup.json')
    const baselineFile = join(root, 'fresh', 'baseline.json')
    /** Starts from one stale report and no `fresh` directory. */
    const seed = () => {
      rmSync(join(root, 'fresh'), { recursive: true, force: true })
      rmSync(reports, { recursive: true, force: true })
      mkdirSync(reports)
      writeFileSync(report, 'stale')
      utimesSync(report, 1, 1)
    }
    /** A run writing `content` as its report and baseline, plus a file outside both locations. */
    const write = (content) => () => {
      writeFileSync(report, content)
      mkdirSync(dirname(baselineFile), { recursive: true })
      writeFileSync(baselineFile, content)
      writeFileSync(join(root, 'fresh', 'unrelated.txt'), content)
      return run()
    }
    let started
    /** The candidate records what it starts from, then writes `content`. */
    const candidate = (content) => () => {
      started = [readFileSync(report, 'utf8'), existsSync(join(root, 'fresh'))]
      return write(content)()
    }
    const spellings = [
      [reports, baselineFile],
      [`${reports}${sep}`, `${root}${sep}x${sep}..${sep}fresh${sep}baseline.json`],
      [relative(process.cwd(), reports) || '.', relative(process.cwd(), baselineFile)],
    ]
    for (const locations of spellings) {
      seed()
      const [baselineRun, candidateRun] = pairRuns(locations, write('same'), candidate('same'))
      assert.deepEqual(started, ['stale', false], String(locations))
      assert.deepEqual([...candidateRun.files.keys()].sort(), [report, baselineFile].sort(), String(locations))
      assert.deepEqual(compareRuns(baselineRun, candidateRun), [], String(locations))
      seed()
      assert.deepEqual(
        compareRuns(...pairRuns(locations, write('a'), candidate('b')))
          .map(({ part }) => part)
          .sort(),
        [`file:${baselineFile}`, `file:${report}`].sort(),
        String(locations),
      )
      seed()
      // Nothing rewritten: the stale report is not output on either side.
      assert.deepEqual(
        pairRuns(
          locations,
          () => run(),
          () => run(),
        ).map(({ files }) => files.size),
        [0, 0],
        String(locations),
      )
    }
  }))

/** Nested locations restore once, from the outer one, so a parent replaced by a link is never written through. */
test('nested locations never restore through a replaced parent', { skip: process.platform === 'win32' }, () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    const outside = join(root, 'outside')
    for (const locations of [
      [join(reports, 'a.json'), reports],
      [reports, join(reports, 'a.json')],
    ]) {
      rmSync(reports, { recursive: true, force: true })
      rmSync(outside, { recursive: true, force: true })
      mkdirSync(reports)
      mkdirSync(outside)
      writeFileSync(join(reports, 'a.json'), 'original')
      writeFileSync(join(outside, 'a.json'), 'outside')
      /** Replaces the report directory with a link to `outside`. */
      const swap = () => {
        rmSync(reports, { recursive: true })
        symlinkSync(outside, reports)
        return run()
      }
      pairRuns(locations, swap, () => run())
      assert.equal(readFileSync(join(outside, 'a.json'), 'utf8'), 'outside', String(locations))
      assert.ok(lstatSync(reports).isDirectory(), String(locations))
      assert.equal(readFileSync(join(reports, 'a.json'), 'utf8'), 'original', String(locations))
    }
  }),
)

/**
 * Red paths: a failing run, snapshot or restore surfaces its error and never lets the candidate start
 * from a changed or half-restored location; a failed baseline is still undone.
 */
test('paired runs fail closed on a failing run, snapshot or restore', () =>
  withTemporary((root) => {
    const reports = join(root, 'reports')
    const report = join(reports, 'vitest-dedup.json')
    /** Starts from one report and no other entries. */
    const seed = () => {
      rmSync(reports, { recursive: true, force: true })
      mkdirSync(reports)
      writeFileSync(report, 'original')
    }
    let candidateRan
    /** A candidate that records that it ran. */
    const candidate = () => {
      candidateRan = true
      return run()
    }
    // [name, baseline, error matcher, candidate expected to run]
    const rows = [
      [
        'baseline throws after writing',
        () => {
          writeFileSync(report, 'partial')
          mkdirSync(join(reports, 'created', 'deep'), { recursive: true })
          throw new Error('baseline failed')
        },
        /baseline failed/,
      ],
    ]
    for (const [name, baseline, error] of rows) {
      seed()
      candidateRan = false
      const expected = describe(reports)
      assert.throws(() => pairRuns([reports], baseline, candidate), error, name)
      assert.equal(candidateRan, false, name)
      assert.deepEqual(describe(reports), expected, name)
    }
    // A failing candidate surfaces its error; the baseline had already been undone when it started.
    seed()
    let started
    assert.throws(
      () =>
        pairRuns(
          [reports],
          () => {
            writeFileSync(report, 'baseline')
            return run()
          },
          () => {
            started = readFileSync(report, 'utf8')
            throw new Error('candidate failed')
          },
        ),
      /candidate failed/,
    )
    assert.equal(started, 'original')
    if (process.platform === 'win32') return
    // An unsupported entry already in a location fails before either run.
    seed()
    assert.equal(spawnSync('mkfifo', [join(reports, 'pipe')]).status, 0)
    let ran = false
    assert.throws(
      () =>
        pairRuns(
          [reports],
          () => {
            ran = true
            return run()
          },
          candidate,
        ),
      /unsupported file type/,
    )
    assert.equal(ran, false)
    // One a baseline creates fails the comparison and is removed by the restore; the candidate never runs.
    seed()
    candidateRan = false
    assert.throws(
      () =>
        pairRuns(
          [reports],
          () => {
            spawnSync('mkfifo', [join(reports, 'made')])
            return run()
          },
          candidate,
        ),
      /unsupported file type/,
    )
    assert.equal(candidateRan, false)
    assert.equal(lstatSync(join(reports, 'made'), { throwIfNoEntry: false }), undefined)
    // A restore that cannot finish (a read-only directory a run left behind) throws; root bypasses the mode.
    if (process.getuid?.() === 0) return
    seed()
    candidateRan = false
    const sealed = join(reports, 'sealed')
    /** Leaves a file in a directory it then makes read-only. */
    const seal = () => {
      mkdirSync(sealed)
      writeFileSync(join(sealed, 'inner.json'), 'x')
      chmodSync(sealed, 0o500)
      return run()
    }
    try {
      // Linux reports EACCES, macOS ENOTEMPTY, for a read-only directory that cannot be emptied.
      assert.throws(() => pairRuns([reports], seal, candidate), /EACCES|EPERM|ENOTEMPTY/)
      assert.equal(candidateRan, false)
    } finally {
      chmodSync(sealed, 0o700)
    }
    // Both errors are kept, the run's first, when a failed baseline also cannot be restored.
    seed()
    /** Leaves a sealed directory, then fails. */
    const sealAndFail = () => {
      seal()
      throw new Error('baseline failed')
    }
    try {
      assert.throws(
        () => pairRuns([reports], sealAndFail, candidate),
        (error) => error instanceof AggregateError && /baseline failed/.test(error.errors[0].message),
      )
    } finally {
      chmodSync(sealed, 0o700)
    }
    // One location that cannot be restored does not stop the others from being restored.
    seed()
    const other = join(root, 'other')
    rmSync(other, { recursive: true, force: true })
    mkdirSync(other)
    writeFileSync(join(other, 'report.json'), 'original')
    /** Seals `reports`, overwrites the other location's report, then fails. */
    const both = () => {
      seal()
      writeFileSync(join(other, 'report.json'), 'partial')
      throw new Error('baseline failed')
    }
    try {
      assert.throws(() => pairRuns([reports, other], both, candidate), AggregateError)
      assert.equal(readFileSync(join(other, 'report.json'), 'utf8'), 'original')
    } finally {
      chmodSync(sealed, 0o700)
    }
  }))

/** Parallel shims each write their own record file; every record is read back whole, however large. */
test('shim records are separate files read back whole', async () => {
  const root = mkdtempSync(join(tmpdir(), 'equivalence-test-'))
  try {
    const large = 'x'.repeat(256 * 1024)
    const script = join(root, 'writer.mjs')
    writeFileSync(
      script,
      `import { writeRecord } from ${JSON.stringify(new URL('./output-equivalence.mjs', import.meta.url).href)};
for (let index = 0; index < 5; index += 1) writeRecord(process.argv[2], { suite: "cli", index, payload: "x".repeat(${large.length}) });`,
    )
    const records = join(root, 'records')
    mkdirSync(records)
    /** Runs one writer process to completion and resolves with its exit code. */
    const writer = () =>
      new Promise((done) => spawn(process.execPath, [script, records], { stdio: 'ignore' }).on('exit', done))
    assert.deepEqual(await Promise.all(Array.from({ length: 6 }, writer)), Array(6).fill(0))
    const read = readRecords(records)
    assert.equal(readdirSync(records).length, 30)
    assert.equal(read.length, 30)
    assert.ok(read.every((record) => record.payload === large))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

/** The version difference reaches the SARIF file only through its own rule, and stdout through its own. */
test('compareRuns reconciles the version in stdout and in a SARIF file, nowhere else', () => {
  const candidate = versionReplacements(['0.7.0', '0.6.0'])
  const sarif = (version) => new Map([['/out/r.sarif', `{\n  "semanticVersion": "${version}",\n  "x": 1\n}`]])
  const html = (version) => new Map([['/out/r.html', `<pre>\nVitestDedup v${version}\n</pre>`]])
  const compare = (baseline, candidateRun) => compareRuns(baseline, candidateRun, { candidate })
  assert.deepEqual(compare(run({ files: sarif('0.6.0') }), run({ files: sarif('0.7.0') })), [])
  assert.deepEqual(compare(run({ stdout: 'VitestDedup v0.6.0\n' }), run({ stdout: 'VitestDedup v0.7.0\n' })), [])
  assert.deepEqual(
    compare(run({ files: html('0.6.0') }), run({ files: html('0.7.0') })).map((d) => d.part),
    ['file:/out/r.html'],
  )
  assert.deepEqual(
    compare(run({ stderr: 'VitestDedup v0.6.0\n' }), run({ stderr: 'VitestDedup v0.7.0\n' })).map((d) => d.part),
    ['stderr'],
  )
})
