import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, realpathSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, delimiter, dirname, join, resolve, sep } from 'node:path'
import { test } from 'node:test'
import { LANES, cleanEnvironment, pause, commands, hookCommands, hookScope, installHooks, main, pushCommits, repositoryKey, runSteps, rustChanged, withLock, withNodeModules, withSnapshot, withTargetCache } from './verify.mjs'
import { coverage } from '../check/check-docstrings.mjs'

const oid = 'a'.repeat(40)
const zero = '0'.repeat(40)

test('CI jobs consuming the docstring parser install locked dependencies before their tests', () => {
  const workflow = readFileSync(resolve('.github/workflows/ci.yml'), 'utf8')
  for (const [job, command] of [['lint', 'test:dev-checks'], ['quality-gates', 'check:coverage:js']]) {
    const section = workflow.split(`\n  ${job}:\n`)[1]?.split(/\n  [\w-]+:\n/)[0]
    assert.ok(section, `${job}: job exists`)
    const install = section.search(/^\s+- run: npm ci --force --ignore-scripts --no-audit --no-fund\s*$/m)
    const consumer = section.indexOf(`- run: npm run ${command}`)
    assert.ok(install >= 0 && consumer > install, `${job}: install dependencies before ${command}`)
  }
})
/**
 * Run a fixture Git command without a shell, capturing output and propagating errors.
 * @param {string} root Fixture repository directory.
 * @param {...string} args Git arguments.
 * @returns {string} Trimmed stdout.
 */
const git = (root, ...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
/**
 * Supply a committed temporary repository to a synchronous test and always remove it.
 * Fixture commits bypass hooks so they cannot invoke the developer's hook configuration.
 * @template T
 * @param {(root: string) => T} body Synchronous fixture test.
 * @returns {T} The test's result; errors propagate after cleanup.
 */
function fixture(body) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'dev-check-test-')))
  try {
    git(root, 'init', '--quiet')
    git(root, 'config', 'user.email', 'checks@example.invalid')
    git(root, 'config', 'user.name', 'Developer Checks')
    writeFileSync(join(root, 'source.txt'), 'committed\n')
    git(root, 'add', 'source.txt')
    git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'fixture')
    return body(root)
  } finally { rmSync(root, { recursive: true, force: true }) }
}

/** Run body with a fake gitleaks first on PATH, so hook tests do not need the installed scanner. */
function withGitleaks(status, body) {
  const bin = mkdtempSync(join(tmpdir(), 'dev-check-gitleaks-'))
  try {
    writeFileSync(join(bin, 'gitleaks'), `#!/bin/sh\necho "fake gitleaks exit ${status}" >&2\nexit ${status}\n`, { mode: 0o755 })
    return body({ ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}` })
  } finally { rmSync(bin, { recursive: true, force: true }) }
}

/**
 * Stage a symlink blob directly, without creating a link in the fixture worktree.
 * @param {string} root Fixture repository directory.
 * @param {string} path Repository-relative index entry.
 * @param {string} target Symlink target stored verbatim in Git.
 * @returns {void}
 */
function stageLink(root, path, target) {
  const blob = execFileSync('git', ['hash-object', '-w', '--stdin'], { cwd: root, input: target, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }).trim()
  git(root, 'update-index', '--add', '--cacheinfo', `120000,${blob},${path}`)
}

for (const [input, expected] of [
  ['', []],
  [`refs/heads/topic ${oid} refs/heads/topic ${zero}\n`, [oid]],
  [`refs/heads/topic ${zero} refs/heads/topic ${oid}`, []],
  [`a ${oid} b ${zero}\na ${oid} c ${oid}`, [oid]],
  [`a ${'b'.repeat(64)} b ${'0'.repeat(64)}`, ['b'.repeat(64)]],
]) test('push refs: '+JSON.stringify(input.slice(0, 20)), () => assert.deepEqual(pushCommits(input), expected))

for (const input of ['a not-an-oid b '+zero, 'a '+oid+' b nope', 'a '+oid+' b', 'extra a '+oid+' b '+zero]) {
  test('reject malformed ref input '+input.slice(0, 18), () => assert.throws(() => pushCommits(input), /Malformed/))
}

test('fast checks are compilation-free; full keeps every gate behind the static ones', () => {
  for (const profile of ['fast', 'full']) assert.deepEqual(commands(profile)[0], ['node', 'scripts/check/check-docstrings.mjs'])
  assert.equal(rustChanged(['fixtures/bad.test.ts']), false)
  for (const path of ['src/model.rs', 'Cargo.lock', 'Cargo.toml', 'scripts/check/check-coverage.mjs']) assert.equal(rustChanged([path]), true)
  // The fast step list is pinned exactly: absence is not provable by filtering for what
  // must not be there (a missing gate makes any "no X" filter pass), so the whole list --
  // fmt gated on rust, nothing that compiles in either spelling -- is the assertion.
  const statics = [
    'node scripts/check/check-docstrings.mjs',
    'npm run check:action-pins',
    'npm run check:release-workflow',
    'npm run check:versions',
    'npm run check:docs',
    'npm run check:names',
    'npm run skills:check',
  ]
  assert.deepEqual(commands('fast', true).map(step => step.join(' ')), [
    ...statics,
    'cargo fmt --all --check',
    'npm run test:dev-checks',
    'npm run test:check-lib',
  ])
  assert.deepEqual(commands('fast', false).map(step => step.join(' ')), [...statics, 'npm run test:dev-checks', 'npm run test:check-lib'])
  const full = commands('full').map(step => step.join(' '))
  assert.ok(full.includes('cargo fmt --all --check'))
  assert.ok(full.includes('cargo clippy --all-targets --all-features --locked'))
  assert.ok(full.includes('cargo doc --no-deps --document-private-items --locked'))
  assert.ok(full.includes('cargo test --all-targets --all-features --locked'))
  assert.ok(full.includes('npm run check'))
  assert.ok(full.includes('npm run check:coverage:rust'))
  assert.equal(commands('full', false).some(step => step.join(' ') === 'npm run check:coverage:rust'), false)
  assert.throws(() => commands('unknown'), /Unknown/)
})

test('full checks front-load the static gates and never repeat a compile-class step', () => {
  const manifest = JSON.parse(readFileSync(new URL('../../package.json', import.meta.url), 'utf8'))
  const chain = manifest.scripts.check.split(' && ')
  const full = commands('full').map(step => step.join(' '))
  // The static gates deliberately run twice: once as the fail-fast prefix, once inside
  // `npm run check` (seconds each, idempotent). Everything that compiles or analyzes a
  // corpus runs exactly once.
  for (const step of ['cargo fmt --all --check', 'cargo clippy --all-targets --all-features --locked', 'cargo doc --no-deps --document-private-items --locked', 'cargo test --all-targets --all-features --locked', 'npm run check', 'npm run check:coverage:rust']) {
    assert.equal(full.filter(other => other === step).length, 1, step + ' runs exactly once')
  }
  for (const gate of ['check:action-pins', 'check:release-workflow', 'check:versions', 'check:docs', 'check:names', 'skills:check']) {
    assert.ok(chain.includes('npm run ' + gate), gate + ' is part of the check chain')
    // Presence first: a missing gate has index -1, which would compare "before" Clippy and
    // pass an ordering-only assertion.
    assert.ok(full.includes('npm run ' + gate), gate + ' leads the full profile')
    // Fail-fast: every static gate precedes the first compile-class step, so its failure
    // costs seconds instead of a dependency-tree build.
    assert.ok(full.indexOf('npm run ' + gate) < full.indexOf('cargo clippy --all-targets --all-features --locked'), gate + ' precedes Clippy')
  }
  assert.ok(commands('fast', false).some(step => step.join(' ') === 'npm run check:names'))
})

test('docstring pre-check uses staged bytes even when the working file has documentation', () => fixture(root => {
  writeFileSync(join(root, 'case.rs'), 'fn missing() {}\n')
  git(root, 'add', 'case.rs')
  writeFileSync(join(root, 'case.rs'), '/// Perform the operation.\nfn missing() {}\n')
  assert.throws(() => main(['pre-commit'], { root, print() {}, check(snapshot, steps) {
    assert.equal(steps[0][1], 'scripts/check/check-docstrings.mjs')
    const report = coverage(snapshot, readFileSync(steps[0][3], 'utf8'))
    assert.equal(report.functions.length, 1)
    if (!report.passed) throw new Error('missing staged docstring')
  } }), /missing staged docstring/)
}))

test('docstring pre-check uses proposed commit bytes despite documented unstaged edits', () => fixture(root => {
  writeFileSync(join(root, 'case.rs'), 'fn missing() {}\n')
  git(root, 'add', 'case.rs')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'fixture missing docs')
  const proposed = git(root, 'rev-parse', 'HEAD')
  writeFileSync(join(root, 'case.rs'), '/// Perform the operation.\nfn missing() {}\n')
  assert.throws(() => main(['pre-push'], { root, input: `refs/heads/topic ${proposed} refs/heads/topic ${zero}`, print() {}, check(snapshot, steps) {
    assert.equal(steps[0][1], 'scripts/check/check-docstrings.mjs')
    const report = coverage(snapshot, readFileSync(steps[0][3], 'utf8'))
    assert.equal(report.functions.length, 1)
    if (!report.passed) throw new Error('missing committed docstring')
  } }), /missing committed docstring/)
}))

test('docstring scope and exported bytes use the same alternate index', () => fixture(root => {
  const env = { ...process.env, GIT_INDEX_FILE: join(root, '.git/alternate-index') }
  execFileSync('git', ['read-tree', 'HEAD'], { cwd: root, env })
  writeFileSync(join(root, 'case.rs'), 'fn missing() {}\n')
  execFileSync('git', ['add', 'case.rs'], { cwd: root, env })
  assert.throws(() => main(['pre-commit'], { root, env, print() {}, check(snapshot, steps) {
    const report = coverage(snapshot, readFileSync(steps[0][3], 'utf8'))
    assert.equal(report.functions.length, 1)
    if (!report.passed) throw new Error('missing alternate-index docstring')
  } }), /missing alternate-index docstring/)
}))

test('snapshot checks the staged bytes and excludes dirty and untracked inputs', () => fixture(root => {
  writeFileSync(join(root, 'source.txt'), 'staged\n')
  git(root, 'add', 'source.txt')
  writeFileSync(join(root, 'source.txt'), 'unstaged\n')
  writeFileSync(join(root, 'private.local.md'), 'not staged')
  let saved
  withSnapshot(root, null, snapshot => {
    saved = snapshot
    assert.equal(readFileSync(join(snapshot, 'source.txt'), 'utf8'), 'staged\n')
    assert.equal(existsSync(join(snapshot, 'private.local.md')), false)
    assert.equal(git(snapshot, 'ls-files'), 'source.txt')
  })
  assert.equal(existsSync(saved), false)
  assert.equal(readFileSync(join(root, 'source.txt'), 'utf8'), 'unstaged\n')
  assert.match(git(root, 'diff', '--cached'), /staged/)
}))

test('snapshot exports skip-worktree entries without changing the original index', () => fixture(root => {
  writeFileSync(join(root, 'omitted.txt'), 'indexed bytes\n')
  git(root, 'add', 'omitted.txt')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'indexed file')
  const commit = git(root, 'rev-parse', 'HEAD')
  git(root, 'update-index', '--skip-worktree', 'omitted.txt')
  rmSync(join(root, 'omitted.txt'))
  const before = git(root, 'ls-files', '--stage', '-v')
  const indexBytes = readFileSync(join(root, '.git/index'))
  assert.match(git(root, 'ls-files', '-v', 'omitted.txt'), /^S /)
  for (const revision of [null, commit]) withSnapshot(root, revision, snapshot => {
    assert.equal(readFileSync(join(snapshot, 'omitted.txt'), 'utf8'), 'indexed bytes\n')
  })
  assert.equal(existsSync(join(root, 'omitted.txt')), false)
  assert.equal(git(root, 'ls-files', '--stage', '-v'), before)
  assert.deepEqual(readFileSync(join(root, '.git/index')), indexBytes)
}))

for (const sparseIndex of [false, true]) test(`sparse-checkout snapshots export files outside the selected directory (sparse index: ${sparseIndex})`, () => fixture(root => {
  for (const directory of ['selected', 'excluded']) {
    mkdirSync(join(root, directory))
    writeFileSync(join(root, directory, 'input.txt'), `${directory} indexed bytes\n`)
  }
  git(root, 'add', '.')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'sparse inputs')
  const commit = git(root, 'rev-parse', 'HEAD')
  git(root, 'sparse-checkout', 'set', sparseIndex ? '--sparse-index' : '--no-sparse-index', 'selected')
  assert.equal(existsSync(join(root, 'excluded/input.txt')), false)
  const before = git(root, 'ls-files', '--stage', '-v')
  const indexBytes = readFileSync(join(root, '.git/index'))
  for (const revision of [null, commit]) withSnapshot(root, revision, snapshot => {
    assert.equal(readFileSync(join(snapshot, 'excluded/input.txt'), 'utf8'), 'excluded indexed bytes\n')
    assert.equal(readFileSync(join(snapshot, 'selected/input.txt'), 'utf8'), 'selected indexed bytes\n')
  })
  assert.equal(existsSync(join(root, 'excluded/input.txt')), false)
  assert.equal(git(root, 'ls-files', '--stage', '-v'), before)
  assert.deepEqual(readFileSync(join(root, '.git/index')), indexBytes)
  assert.equal(git(root, 'sparse-checkout', 'list'), 'selected')
}))

test('push snapshot checks the proposed older commit rather than current HEAD/index', () => fixture(root => {
  const old = git(root, 'rev-parse', 'HEAD')
  writeFileSync(join(root, 'source.txt'), 'new commit\n')
  git(root, 'add', 'source.txt')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'later')
  writeFileSync(join(root, 'source.txt'), 'staged\n')
  git(root, 'add', 'source.txt')
  withSnapshot(root, old, snapshot => assert.equal(readFileSync(join(snapshot, 'source.txt'), 'utf8'), 'committed\n'))
  assert.match(git(root, 'diff', '--cached'), /staged/)
}))

for (const kind of ['absolute', 'relative', 'chain', 'directory', 'dangling', 'cycle']) {
  test(`snapshot rejects ${kind} symlinks before checks run`, { skip: process.platform === 'win32' && 'Git symlink checkout requires Windows symlink privileges' }, () => fixture(root => {
    const external = join(root, 'source.txt')
    const target = kind === 'relative' ? `../../${basename(root)}/source.txt`
      : kind === 'directory' ? root
      : kind === 'dangling' ? 'missing.txt'
      : kind === 'cycle' ? 'Cargo.toml'
      : external
    if (kind === 'chain') stageLink(root, 'alias', external)
    stageLink(root, 'Cargo.toml', kind === 'chain' ? 'alias' : target)
    git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'staged links')
    const commit = git(root, 'rev-parse', 'HEAD')
    writeFileSync(external, 'unstaged external bytes\n')
    const before = git(root, 'status', '--porcelain')
    for (const revision of [null, commit]) {
      let called = false
      assert.throws(() => withSnapshot(root, revision, () => { called = true }), /Snapshot symlink/)
      assert.equal(called, false)
    }
    assert.equal(readFileSync(external, 'utf8'), 'unstaged external bytes\n')
    assert.equal(git(root, 'status', '--porcelain'), before)
  }))
}

test('snapshot internal symlinks read staged bytes through nested directories and chains', { skip: process.platform === 'win32' && 'Git symlink checkout requires Windows symlink privileges' }, () => fixture(root => {
  stageLink(root, 'nested/alias', '../source.txt')
  stageLink(root, 'Cargo.toml', 'nested/alias')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'internal links')
  const commit = git(root, 'rev-parse', 'HEAD')
  writeFileSync(join(root, 'source.txt'), 'unstaged bytes\n')
  for (const revision of [null, commit]) withSnapshot(root, revision, snapshot => {
    assert.equal(readFileSync(join(snapshot, 'Cargo.toml'), 'utf8'), 'committed\n')
  })
}))

test('failing snapshot check restores state and removes temporary source', () => fixture(root => {
  let saved
  assert.throws(() => withSnapshot(root, null, snapshot => { saved = snapshot; throw new Error('rejected') }), /rejected/)
  assert.equal(existsSync(saved), false)
  assert.equal(git(root, 'status', '--porcelain'), '')
  assert.throws(() => withSnapshot(root, 'does-not-exist', () => assert.fail()), /Command failed/)
}))

test('two lanes run at once, a third is refused, and lanes are removed on success and failure', () => fixture(root => {
  const common = join(root, '.git')
  assert.equal(LANES, 2)
  withLock(common, first => {
    assert.equal(first, 0)
    assert.match(readFileSync(join(common, 'delivery-check.lock/owner'), 'utf8'), /^pid=/)
    withLock(common, second => {
      assert.equal(second, 1)
      assert.match(readFileSync(join(common, 'delivery-check.lock.1/owner'), 'utf8'), /^pid=/)
      assert.throws(() => withLock(common, () => assert.fail()), /every lane.*delivery-check\.lock, .*delivery-check\.lock\.1\./)
    })
    assert.equal(existsSync(join(common, 'delivery-check.lock.1')), false)
  })
  assert.equal(existsSync(join(common, 'delivery-check.lock')), false)
  assert.throws(() => withLock(common, () => { throw new Error('failed') }), /failed/)
  assert.equal(existsSync(join(common, 'delivery-check.lock')), false)
}))

test('the default pause blocks for the time it is given', () => {
  const start = performance.now()
  pause(20)
  assert.ok(performance.now() - start >= 15)
})

test('a second hook run gets its own export and cache, and a manual run never takes lane 1', () => fixture(root => {
  const common = join(root, '.git')
  const env = { ...process.env }
  delete env.CARGO_TARGET_DIR
  delete env.CARGO_LLVM_COV_TARGET_DIR
  const key = repositoryKey(common)
  const seen = []
  const check = (snapshot, _steps, options) => { seen.push([snapshot, options.env.CARGO_TARGET_DIR, options.env.CARGO_BUILD_BUILD_DIR]) }
  writeFileSync(join(root, 'test.rs'), 'fn main() {}\n'); git(root, 'add', 'test.rs')
  main(['pre-commit'], { root, env, check, print: () => {} })
  withLock(common, () => main(['pre-commit'], { root, env, check, print: () => {} }))
  const [[snapshot0, cache0], [snapshot1, cache1, build1]] = seen
  assert.equal(snapshot0, join(env.XDG_RUNTIME_DIR || tmpdir(), `vitest-dedup-hook-${key}`, 'source'))
  assert.equal(snapshot1, join(env.XDG_RUNTIME_DIR || tmpdir(), `vitest-dedup-hook-${key}-1`, 'source'))
  assert.equal(cache0, join(root, 'target/hook-snapshot', key))
  assert.equal(cache1, join(root, 'target/hook-snapshot', `${key}-1`))
  assert.equal(build1, cache1)
  mkdirSync(join(common, 'delivery-check.lock'))
  const lanes = []
  assert.throws(() => withLock(common, () => assert.fail(), { lanes: 1 }), /every lane/)
  withLock(common, lane => lanes.push(lane))
  assert.deepEqual(lanes, [1])
  rmSync(join(common, 'delivery-check.lock'), { recursive: true })
}))

test('a run with one lane allowed takes only lane 0', () => fixture(root => {
  const common = join(root, '.git')
  withLock(common, () => {
    assert.throws(() => withLock(common, () => assert.fail(), { lanes: 1 }), /every lane/)
  })
}))

test('a busy repository queues instead of refusing, and gives up after its wait', () => fixture(root => {
  const common = join(root, '.git')
  const printed = []
  withLock(common, () => withLock(common, () => {
    let naps = 0
    const sleep = () => { if (++naps === 2) rmSync(join(common, 'delivery-check.lock.1'), { recursive: true }) }
    assert.equal(withLock(common, lane => `lane ${lane}`, { waitMs: 60_000, sleep, print: line => printed.push(line) }), 'lane 1')
    assert.equal(naps, 2)
    mkdirSync(join(common, 'delivery-check.lock.1'))
    let spent = 0
    assert.throws(() => withLock(common, () => assert.fail(), { waitMs: 10_000, sleep: () => { spent++ } }), /every lane/)
    assert.equal(spent, 2)
  }))
  assert.deepEqual(printed, ['All 2 verification lanes are busy; waiting.'])
}))

test('hook environment removes original Git paths while preserving deliberate build flags', () => {
  const env = cleanEnvironment({ GIT_DIR: '/wrong', GIT_WORK_TREE: '/wrong', GIT_INDEX_FILE: '/wrong', GIT_COMMON_DIR: '/wrong', GIT_PREFIX: 'wrong', GIT_OBJECT_DIRECTORY: '/wrong', GIT_ALTERNATE_OBJECT_DIRECTORIES: '/wrong', GIT_CONFIG_PARAMETERS: "'core.hooksPath'='/wrong'", GIT_CONFIG_COUNT: '1', GIT_CONFIG_KEY_0: 'core.hooksPath', GIT_CONFIG_VALUE_0: '/wrong', PATH: 'preserved' })
  assert.deepEqual(env, { PATH: 'preserved' })
})

for (const [result, expected] of [[{ status: 1 }, /exit 1/], [{ status: null, signal: 'SIGTERM' }, /SIGTERM/], [{ error: new Error('missing tool') }, /missing tool/]]) {
  test('failed commands stop with bounded diagnostics '+String(expected), () => fixture(root => {
    const output = []
    let calls = 0
    assert.throws(() => runSteps(root, [['cargo', 'bad'], ['npm', 'never']], {
      logRoot: join(root, 'logs'), print: line => output.push(line),
      spawn: (_cmd, _args, options) => { calls++; writeFileSync(options.stdio[1], Array.from({ length: 200 }, (_, i) => `line ${i}`).join('\n')); return result },
    }), expected)
    assert.equal(calls, 1)
    assert.equal(output.length, 1)
    assert.equal(output[0].split('\n').length, 100)
    assert.match(output[0], /^line 100\n/)
  }))
}

test('successful command uses CI warnings and preserves supplied flags without streaming', () => fixture(root => {
  const output = []
  runSteps(root, [['cargo', 'clippy']], {
    logRoot: join(root, 'logs'), env: { RUSTFLAGS: '-C debuginfo=0', GIT_DIR: '/wrong' }, print: line => output.push(line),
    spawn: (cmd, args, options) => {
      assert.equal(cmd, 'cargo'); assert.deepEqual(args, ['clippy'])
      assert.equal(options.env.RUSTFLAGS, '-C debuginfo=0 -D warnings')
      assert.equal(options.env.RUSTDOCFLAGS, '-D warnings')
      assert.equal(options.env.GIT_DIR, undefined)
      assert.equal(options.stdio[1], options.stdio[2])
      return { status: 0 }
    },
  })
  assert.deepEqual(output, ['PASS cargo clippy'])
}))

test('installer respects existing hook managers and installs only for the current worktree', () => fixture(root => {
  assert.throws(() => installHooks(root), /Missing hook/)
  mkdirSync(join(root, '.githooks'))
  for (const name of ['pre-commit', 'pre-push']) writeFileSync(join(root, '.githooks', name), '#!/bin/sh\n')
  git(root, 'config', 'core.hooksPath', '/someone-else/hooks')
  assert.throws(() => installHooks(root), /preserved/)
  assert.equal(git(root, 'config', '--get', 'core.hooksPath'), '/someone-else/hooks')
  git(root, 'config', '--unset', 'core.hooksPath')
  const hookPath = installHooks(root)
  assert.equal(git(root, 'config', '--worktree', '--get', 'core.hooksPath'), hookPath)
  assert.equal(git(root, 'config', '--local', '--get', 'extensions.worktreeConfig'), 'true')
  assert.equal(installHooks(root), hookPath)
}))

test('pre-commit checks staged Rust, pre-push checks the specified commit, and deletion skips', () => fixture(root => {
  const output = []; const snapshots = []
  const env = { ...process.env }
  delete env.CARGO_TARGET_DIR
  delete env.CARGO_LLVM_COV_TARGET_DIR
  const hookCache = join(root, 'target/hook-snapshot', repositoryKey(join(root, '.git')))
  const check = (snapshot, steps, options) => {
    snapshots.push(snapshot)
    assert.equal(options.env.CARGO_TARGET_DIR, hookCache)
    assert.equal(options.env.CARGO_LLVM_COV_TARGET_DIR, join(hookCache, 'llvm-cov-target'))
    // Pre-commit compiles nothing (fmt only); pre-push runs the CI-like flow with Clippy.
    assert.ok(steps.some(step => step[1] === 'fmt'))
    if (snapshots.length === 1) assert.ok(!steps.some(step => step[1] === 'clippy'))
    else assert.ok(steps.some(step => step[1] === 'clippy'))
  }
  writeFileSync(join(root, 'test.rs'), 'fn main() {}\n'); git(root, 'add', 'test.rs')
  main(['pre-commit'], { root, env, check, print: line => output.push(line) })
  const revision = git(root, 'rev-parse', 'HEAD')
  main(['pre-push', 'origin', 'url'], { root, env, input: `ref ${revision} remote ${zero}`, check, print: line => output.push(line) })
  assert.equal(snapshots.length, 2)
  assert.ok(snapshots.every(p => !existsSync(p)))
  assert.equal(snapshots[0], snapshots[1])
  main(['pre-push'], { root, env, input: `ref ${zero} remote ${revision}`, check: () => assert.fail(), print: line => output.push(line) })
  assert.match(output.at(-1), /No commits/)
}))

test('hook snapshots build beneath the configured cache in a directory the checkout never uses', () => fixture(root => {
  // A snapshot's test binary bakes the export path; sharing the checkout's cache hands that
  // binary to a later checkout run after the export is gone (backlog D4). The coverage directory
  // is derived from the hook cache too, an instrumented binary being the same artifact class, and
  // so is Cargo's separate build directory: a shared one holds the fingerprints and deps that make
  // the target directory split meaningless (measured with a toy crate, cargo 1.98.1).
  const shared = join(root, 'shared-build')
  const env = { ...process.env, CARGO_TARGET_DIR: 'cache', CARGO_LLVM_COV_TARGET_DIR: 'coverage', CARGO_BUILD_BUILD_DIR: shared, CARGO_LLVM_COV_BUILD_DIR: shared }
  const key = repositoryKey(join(root, '.git'))
  const hookCache = join(root, 'cache/hook-snapshot', key)
  writeFileSync(join(root, 'test.rs'), 'fn main() {}\n'); git(root, 'add', 'test.rs')
  let checked = false
  main(['pre-commit'], { root, env, print: () => {}, check: (snapshot, _steps, options) => {
    checked = true
    // The export stays outside the checkout, where no unstaged Cargo config or ignore file of the
    // caller's is an ancestor, in the private runtime directory when the platform has one.
    assert.equal(snapshot, join(env.XDG_RUNTIME_DIR || tmpdir(), `vitest-dedup-hook-${key}`, 'source'))
    assert.equal(options.env.CARGO_TARGET_DIR, hookCache)
    assert.equal(options.env.CARGO_BUILD_BUILD_DIR, hookCache)
    assert.equal(options.env.CARGO_LLVM_COV_TARGET_DIR, join(hookCache, 'llvm-cov-target'))
    assert.equal(options.env.CARGO_LLVM_COV_BUILD_DIR, join(hookCache, 'llvm-cov-target'))
    assert.equal(realpathSync(join(snapshot, 'target')), realpathSync(hookCache))
    assert.notEqual(realpathSync(join(snapshot, 'target')), realpathSync(join(root, 'cache')))
    // The export is private, as `mkdtemp`'s directory was before the path became fixed.
    if (process.platform !== 'win32') assert.equal(statSync(dirname(snapshot)).mode & 0o777, 0o700)
  } })
  assert.ok(checked)
  assert.ok(existsSync(hookCache), 'the hook cache outlives the run so dependencies stay warm')
  assert.equal(existsSync(join(root, 'coverage')), false)
  assert.equal(existsSync(join(root, 'shared-build')), false)
}))

test('one repository exports every hook snapshot to one cleared directory and one cache; another repository gets its own', () => fixture(root => fixture(other => {
  const seen = []; const caches = []
  const run = (where) => main(['pre-commit'], { root: where, env: { ...process.env, CARGO_TARGET_DIR: join(root, 'external') }, print: () => {}, check: (snapshot, _steps, options) => {
    seen.push(snapshot); caches.push(options.env.CARGO_TARGET_DIR)
    assert.equal(existsSync(join(snapshot, 'leftover')), false, 'an interrupted run leaves nothing behind for the next export')
  } })
  run(root)
  // Simulate an interrupted run: the export directory survives with a file no checkout tracks.
  mkdirSync(seen[0], { recursive: true })
  writeFileSync(join(seen[0], 'leftover'), 'from a killed run')
  run(root)
  run(other)
  assert.equal(seen[0], seen[1])
  assert.notEqual(seen[0], seen[2])
  assert.ok(seen.every(p => !existsSync(p)))
  // Two clones pointed at one external cache still build apart: Cargo would otherwise let the
  // clone with older files reuse the other clone's binary once that export is gone.
  assert.equal(caches[0], caches[1])
  assert.notEqual(caches[0], caches[2])
  for (const cache of caches) assert.ok(cache.startsWith(join(root, 'external', 'hook-snapshot') + sep), cache)
})))

test('hook exports prefer the private runtime directory and fail clearly when the fixed directory is not ours', { skip: process.platform === 'win32' && 'POSIX permissions' }, () => fixture(root => {
  const runtime = join(root, 'runtime')
  mkdirSync(runtime)
  const env = { ...process.env, XDG_RUNTIME_DIR: runtime }
  let snapshot
  main(['pre-commit'], { root, env, print: () => {}, check: s => { snapshot = s } })
  assert.equal(dirname(dirname(snapshot)), runtime)
  if (typeof process.getuid === 'function' && process.getuid() === 0) return
  // A directory at the fixed path that cannot be cleared stands in for one another user owns:
  // the run must name it and the remedy rather than export beside or into it.
  const taken = dirname(snapshot)
  mkdirSync(join(taken, 'held'), { recursive: true })
  chmodSync(taken, 0o500)
  try {
    assert.throws(() => main(['pre-commit'], { root, env, print: () => {}, check: () => assert.fail('exported into a directory it could not prepare') }), /Cannot prepare the hook export directory .*XDG_RUNTIME_DIR or TMPDIR/)
  } finally { chmodSync(taken, 0o700) }
}))

test('manual profiles preserve configured caches; invalid usage fails', () => fixture(root => {
  main(['fast'], { root, env: { ...process.env, CARGO_TARGET_DIR: 'cache', CARGO_LLVM_COV_TARGET_DIR: 'coverage' }, print: () => {}, check: (cwd, _steps, options) => {
    assert.equal(cwd, root)
    assert.equal(options.env.CARGO_TARGET_DIR, join(root, 'cache'))
    assert.equal(options.env.CARGO_LLVM_COV_TARGET_DIR, join(root, 'coverage'))
  } })
  for (const args of [[], ['nope'], ['fast', 'extra']]) assert.throws(() => main(args, { root }), /Usage/)
}))

test('CLI reports invalid usage and exits nonzero', () => {
  const run = spawnSync(process.execPath, [resolve('scripts/dev/verify.mjs'), 'invalid'], { encoding: 'utf8' })
  assert.equal(run.status, 1)
  assert.match(run.stderr, /Usage/)
})


test('deleting staged Rust still counts as a Rust change', () => fixture(root => {
  writeFileSync(join(root, 'removed.rs'), 'fn main() {}\n')
  git(root, 'add', 'removed.rs')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'Rust fixture')
  git(root, 'rm', 'removed.rs')
  main(['pre-commit'], { root, print: () => {}, check: (snapshot, steps) => {
    assert.equal(existsSync(join(snapshot, 'removed.rs')), false)
    // The deletion is a Rust change, so commit-time still formats; Clippy belongs to push.
    assert.ok(steps.some(step => step[1] === 'fmt'))
  } })
}))

test('main installer routes to worktree configuration', () => fixture(root => {
  mkdirSync(join(root, '.githooks'))
  for (const name of ['pre-commit', 'pre-push']) writeFileSync(join(root, '.githooks', name), '#!/bin/sh\n')
  const output = []
  main(['install'], { root, print: line => output.push(line) })
  assert.equal(git(root, 'config', '--worktree', '--get', 'core.hooksPath'), join(root, '.githooks'))
  assert.match(output[0], /Hooks installed/)
}))


test('cache alias reaches the built binary, is cleaned after failure, and preserves existing directories', () => fixture(root => {
  const cache = join(root, 'cache')
  mkdirSync(join(cache, 'release'), { recursive: true })
  writeFileSync(join(cache, 'release', 'built-binary'), 'current source')
  assert.throws(() => withTargetCache(root, cache, () => {
    assert.equal(readFileSync(join(root, 'target/release/built-binary'), 'utf8'), 'current source')
    throw new Error('gate failed')
  }), /gate failed/)
  assert.equal(existsSync(join(root, 'target')), false)
  withTargetCache(root, cache, () => assert.ok(existsSync(join(root, 'target'))))
  assert.equal(existsSync(join(root, 'target')), false)
  mkdirSync(join(root, 'target'))
  assert.throws(() => withTargetCache(root, cache, () => assert.fail()), /conflicts/)
  assert.ok(existsSync(join(root, 'target')))
  withTargetCache(root, join(root, 'target'), () => assert.ok(existsSync(join(root, 'target'))))
  assert.ok(existsSync(join(root, 'target')))
}))

for (const typeChange of [false, true]) test(`staged ${typeChange ? 'type changes' : 'Unicode paths'} still run Rust checks`, () => fixture(root => {
  const path = typeChange ? 'changed.rs' : '日本語.rs'
  writeFileSync(join(root, path), 'fn main() {}\n')
  git(root, 'add', path)
  if (typeChange) {
    git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'Rust source')
    const blob = execFileSync('git', ['hash-object', '-w', '--stdin'], { cwd: root, input: 'source.txt', encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }).trim()
    git(root, 'update-index', '--cacheinfo', `120000,${blob},${path}`)
  }
  main(['pre-commit'], { root, print: () => {}, check: (_snapshot, steps) => assert.ok(steps.some(step => step[0] === 'cargo')) })
}))

test('snapshots see the checkout\'s node_modules only when it exists, and the alias never outlives the check', () => fixture(root => {
  // `npm run check` inside a snapshot reaches the title harness, which spawns the pinned Vitest
  // from node_modules; a snapshot of tracked files alone would fail every hook-driven push.
  const env = { ...process.env }
  delete env.CARGO_TARGET_DIR
  delete env.CARGO_LLVM_COV_TARGET_DIR
  const seen = []
  main(['pre-commit'], { root, env, print: () => {}, check: snapshot => seen.push(existsSync(join(snapshot, 'node_modules'))) })
  mkdirSync(join(root, 'node_modules'))
  writeFileSync(join(root, 'node_modules', 'marker'), 'installed\n')
  main(['pre-commit'], { root, env, print: () => {}, check: snapshot => seen.push(readFileSync(join(snapshot, 'node_modules', 'marker'), 'utf8')) })
  assert.deepEqual(seen, [false, 'installed\n'])
  const snapshot = join(root, 'snap')
  mkdirSync(snapshot)
  assert.throws(() => withNodeModules(snapshot, root, () => {
    assert.equal(readFileSync(join(snapshot, 'node_modules', 'marker'), 'utf8'), 'installed\n')
    throw new Error('gate failed')
  }), /gate failed/)
  assert.equal(existsSync(join(snapshot, 'node_modules')), false)
  assert.equal(readFileSync(join(root, 'node_modules', 'marker'), 'utf8'), 'installed\n')
}))

test('nested target caches are refused without creating checkout directories', () => fixture(root => {
  assert.throws(() => withTargetCache(root, join(root, 'target/custom'), () => assert.fail()), /target/)
  assert.equal(existsSync(join(root, 'target')), false)
}))

for (const profile of ['fast', 'full']) test(`${profile} uses a fresh default target without a symlink and preserves build output on failure`, () => fixture(root => {
  const target = join(root, 'target')
  const env = { ...process.env }
  delete env.CARGO_TARGET_DIR
  assert.equal(existsSync(target), false)
  let checked = false
  main([profile], { root, env, print: () => {}, check: (_root, _steps, options) => {
    checked = true
    assert.equal(options.env.CARGO_TARGET_DIR, target)
    assert.ok(lstatSync(target).isDirectory())
  } })
  assert.ok(checked)
  rmSync(target, { recursive: true })
  assert.throws(() => main([profile], { root, env, print: () => {}, check: () => {
    assert.ok(lstatSync(target).isDirectory())
    writeFileSync(join(target, 'built-output'), 'preserved')
    throw new Error('gate failed')
  } }), /gate failed/)
  assert.equal(readFileSync(join(target, 'built-output'), 'utf8'), 'preserved')
}))

test('conflicting absent caches are refused without creating directories or changing the target', () => fixture(root => {
  const target = join(root, 'target')
  const cache = join(root, 'absent/cache')
  mkdirSync(target)
  writeFileSync(join(target, 'sentinel'), 'preserved')
  assert.throws(() => withTargetCache(root, cache, () => assert.fail()), /conflicts/)
  assert.equal(existsSync(join(root, 'absent')), false)
  assert.equal(readFileSync(join(target, 'sentinel'), 'utf8'), 'preserved')
}))

for (const [from, to, rust] of [
  ['old.rs', 'new.txt', true],
  ['old.txt', 'new.rs', true],
  ['old.rs', 'new.rs', true],
  ['old.txt', 'new.txt', false],
  ['Cargo.toml', 'saved.txt', true],
  ['scripts/check/check-coverage.mjs', 'saved.mjs', true],
]) test(`staged rename ${from} to ${to} selects the required checks`, () => fixture(root => {
  mkdirSync(join(root, 'scripts/check'), { recursive: true })
  writeFileSync(join(root, from), 'unchanged content\n')
  git(root, 'add', from)
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'rename source')
  git(root, 'config', 'diff.renames', 'true')
  git(root, 'mv', from, to)
  assert.match(git(root, 'diff', '--cached', '--name-status'), /^R100\s/)
  main(['pre-commit'], { root, print: () => {}, check: (_snapshot, steps) => {
    assert.equal(steps.some(step => step[0] === 'cargo'), rust)
  } })
}))

for (const mode of ['pre-commit', 'pre-push']) test(`${mode} loads the verifier from the snapshot despite unstaged replacement`, () => fixture(root => {
  mkdirSync(join(root, 'scripts/dev'), { recursive: true })
  writeFileSync(join(root, 'scripts/dev/verify.mjs'), "console.error('snapshot verifier rejected'); process.exit(7)\n")
  git(root, 'add', 'scripts/dev/verify.mjs')
  const hooks = resolve('.githooks')
  let args
  if (mode === 'pre-commit') args = ['-c', `core.hooksPath=${hooks}`, 'commit', '-m', 'must reject']
  else {
    git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'proposed verifier')
    const proposed = git(root, 'rev-parse', 'HEAD')
    git(root, 'init', '--bare', '--quiet', 'remote.git')
    args = ['-c', `core.hooksPath=${hooks}`, 'push', join(root, 'remote.git'), `${proposed}:refs/heads/topic`]
  }
  writeFileSync(join(root, 'scripts/dev/verify.mjs'), 'process.exit(0)\n')
  const run = withGitleaks(0, env => spawnSync('git', args, { cwd: root, encoding: 'utf8', env }))
  assert.notEqual(run.status, 0, run.stderr)
  assert.match(run.stderr, /snapshot verifier rejected/)
}))

test('pre-commit stops at a gitleaks finding before the verifier runs', () => fixture(root => {
  mkdirSync(join(root, 'scripts/dev'), { recursive: true })
  writeFileSync(join(root, 'scripts/dev/verify.mjs'), "console.error('snapshot verifier rejected'); process.exit(7)\n")
  git(root, 'add', 'scripts/dev/verify.mjs')
  const hooks = resolve('.githooks')
  const run = withGitleaks(1, env => spawnSync('git', ['-c', `core.hooksPath=${hooks}`, 'commit', '-m', 'must reject'], { cwd: root, encoding: 'utf8', env }))
  assert.notEqual(run.status, 0)
  assert.match(run.stderr, /fake gitleaks exit 1/)
  assert.doesNotMatch(run.stderr, /snapshot verifier rejected/)
}))

test('push hook uses each distinct proposed driver and skips deletion-only refs', () => fixture(root => {
  mkdirSync(join(root, 'scripts/dev'), { recursive: true })
  const revisions = []
  for (const name of ['first', 'second']) {
    writeFileSync(join(root, 'scripts/dev/verify.mjs'), `console.error('${name} proposed driver'); process.exit(0)\n`)
    git(root, 'add', 'scripts/dev/verify.mjs')
    git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', name)
    revisions.push(git(root, 'rev-parse', 'HEAD'))
  }
  writeFileSync(join(root, 'scripts/dev/verify.mjs'), 'process.exit(9)\n')
  git(root, 'init', '--bare', '--quiet', 'remote.git')
  const base = ['-c', `core.hooksPath=${resolve('.githooks')}`, 'push', join(root, 'remote.git')]
  const run = spawnSync('git', [...base, `${revisions[0]}:refs/heads/a`, `${revisions[0]}:refs/heads/b`, `${revisions[1]}:refs/heads/c`], { cwd: root, encoding: 'utf8' })
  assert.equal(run.status, 0, run.stderr)
  for (const name of ['first', 'second']) assert.equal(run.stderr.split(`${name} proposed driver`).length - 1, 1, run.stderr)
  const deletion = spawnSync('git', [...base, '--delete', 'a', 'b', 'c'], { cwd: root, encoding: 'utf8' })
  assert.equal(deletion.status, 0, deletion.stderr)
  assert.doesNotMatch(deletion.stderr, /proposed driver/)
}))

test('CLI entry runs through a directory alias rather than silently skipping checks', () => fixture(root => {
  const driver = join(root, 'driver')
  mkdirSync(driver)
  writeFileSync(join(driver, 'verify.mjs'), readFileSync(resolve('scripts/dev/verify.mjs')))
  withTargetCache(root, driver, () => {
    const run = spawnSync(process.execPath, [join(root, 'target/verify.mjs'), 'invalid'], { encoding: 'utf8' })
    assert.equal(run.status, 1, run.stderr)
    assert.match(run.stderr, /Usage/)
  })
}))

test('existing CI Rust flags keep their Cargo cache identity without weakening warnings', () => fixture(root => {
  for (const [flags, expected] of [
    ['-D warnings', '-D warnings'],
    ['-Dwarnings', '-Dwarnings'],
    ['-C debuginfo=0 -D warnings', '-C debuginfo=0 -D warnings'],
    ['-D warnings -A warnings', '-D warnings -A warnings -D warnings'],
  ]) {
    runSteps(root, [['cargo', 'clippy']], {
      logRoot: join(root, 'logs'), env: { RUSTFLAGS: flags }, print: () => {},
      spawn: (_cmd, _args, options) => { assert.equal(options.env.RUSTFLAGS, expected); assert.ok(/-D\s*warnings/.test(options.env.RUSTDOCFLAGS)); return { status: 0 } },
    })
  }
}))

/** Commit exact fixture files without invoking the developer's configured hooks. */
function commitChanges(root, files) {
  for (const [path, contents] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true })
    writeFileSync(join(root, path), contents)
    git(root, 'add', '--', path)
  }
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'fixture change')
  return git(root, 'rev-parse', 'HEAD')
}

/** Capture the real hook dispatch plan over exported fixture bytes without running heavy tools. */
function plannedChecks(root, mode, input) {
  const plans = []
  main([mode], { root, input, print() {}, check(_snapshot, steps) { plans.push(steps.map(step => step.join(' '))) } })
  return plans
}

for (const mode of ['pre-commit', 'pre-push']) test(`${mode}: documentation-only changes select static checks without compiler or unrelated suites`, () => fixture(root => {
  const before = git(root, 'rev-parse', 'HEAD')
  const proposed = commitChanges(root, { 'README.md': '# Documentation\n' })
  if (mode === 'pre-commit') {
    writeFileSync(join(root, 'README.md'), '# Updated documentation\n')
    git(root, 'add', 'README.md')
  }
  const [steps] = plannedChecks(root, mode, `refs/heads/topic ${proposed} refs/heads/topic ${before}`)
  assert.ok(steps.includes('npm run check:docs'))
  assert.ok(steps.includes('npm run check:names'))
  for (const step of steps) assert.doesNotMatch(step, /^(cargo |npm (test$|run (check$|check:coverage|check:corpus|check:recall|test:)))/, step)
}))

test('incremental docs push skips earlier Rust work, but a new branch includes it', () => fixture(root => {
  const base = git(root, 'rev-parse', 'HEAD')
  git(root, 'update-ref', 'refs/remotes/origin/main', base)
  const remote = commitChanges(root, { 'src/lib.rs': 'pub fn value() {}\n' })
  const proposed = commitChanges(root, { 'README.md': '# Documentation\n' })
  const [incremental] = plannedChecks(root, 'pre-push', `refs/heads/topic ${proposed} refs/heads/topic ${remote}`)
  assert.equal(incremental.some(step => step.startsWith('cargo ')), false)
  const [initial] = plannedChecks(root, 'pre-push', `refs/heads/topic ${proposed} refs/heads/topic ${zero}`)
  assert.ok(initial.includes('npm run check:coverage:rust'))
  assert.ok(initial.includes('cargo test --all-targets --all-features --locked'))
}))

for (const [paths, scopes, required] of [
  [['docs/development.md', 'skills/vitest-dedup/SKILL.md'], ['docs'], ['npm run check:docs', 'npm run skills:check']],
  [['.github/actions/vitest-dedup/index.js'], ['action'], ['npm run test:action']],
  [['npm/lib/launcher.mjs'], ['npm'], ['npm test', 'npm run pack:check']],
  [['scripts/dev/verify.mjs', '.githooks/pre-push'], ['tooling'], ['npm run test:dev-checks', 'npm run test:check-lib']],
  [['README.md', 'npm/test/launcher.test.mjs', '.github/actions/vitest-dedup/action.yml'], ['action', 'docs', 'npm'], ['npm run test:action', 'npm test', 'npm run pack:check']],
]) test(`local scope ${scopes.join('+')} selects relevant checks and preserves JS ratchets`, () => {
  assert.deepEqual(hookScope(paths), scopes)
  for (const mode of ['pre-commit', 'pre-push']) {
    const steps = hookCommands(mode, paths, 'scope.diff').map(step => step.join(' '))
    assert.equal(steps[0], 'node scripts/check/check-docstrings.mjs --diff-file scope.diff')
    for (const step of required) assert.ok(steps.includes(step), step)
    assert.equal(steps.some(step => step.startsWith('cargo ')), false)
    const quality = mode === 'pre-push' && scopes.some(scope => scope !== 'docs')
    for (const gate of ['check:duplication', 'check:coverage:js']) assert.equal(steps.includes('npm run ' + gate), quality)
    assert.equal(new Set(steps).size, steps.length)
  }
})

/** The pre-push checks `paths` select, as command lines. */
const prePush = paths => hookCommands('pre-push', paths, 'scope.diff').map(step => step.join(' '))
const SLOW = ['cargo clippy', 'cargo test', 'cargo doc', 'cargo build', 'npm run check:coverage', 'npm run check:duplication', 'npm run check:corpus']

test('a workflow change compiles nothing and runs no slow gate', () => {
  for (const path of ['.github/workflows/ci.yml', '.github/workflows/mutants.yml', '.github/dependabot.yml']) {
    assert.deepEqual(hookScope([path]), ['ci'])
    const steps = prePush([path])
    assert.ok(steps.includes('npm run check:action-pins') && steps.includes('npm run check:release-workflow'))
    assert.ok(steps.includes('npm run test:dev-checks'))
    assert.ok(steps.includes('npm test'), 'npm/test reads the workflows')
    assert.equal(steps.some(step => SLOW.some(slow => step.startsWith(slow))), false, steps.join())
  }
})

test('documentation runs static checks only, at both hooks', () => {
  for (const mode of ['pre-commit', 'pre-push']) {
    const steps = hookCommands(mode, ['docs/development.md', 'README.md'], 'scope.diff').map(step => step.join(' '))
    assert.equal(steps.some(step => step.startsWith('cargo') || step.includes('test:')), false, steps.join())
  }
})

test('a gate script runs the corpus gates and no Rust suite', () => {
  assert.deepEqual(hookScope(['scripts/check/lib/collection-oracle.mjs']), ['gates'])
  const steps = prePush(['scripts/check/lib/collection-oracle.mjs'])
  for (const gate of ['check:corpus', 'check:recall', 'check:oracle', 'check:next-duplicates', 'check:coverage:js']) assert.ok(steps.includes(`npm run ${gate}`), gate)
  assert.ok(steps.includes('cargo build --release --locked'))
  assert.equal(steps.some(step => /^cargo (?:test|clippy|doc)/.test(step)), false, steps.join())
  assert.equal(steps.includes('npm run check:coverage:rust'), false)
})

test('a fixture runs the corpus gates and the Rust tests that read it, and no lints or coverage', () => {
  assert.deepEqual(hookScope(['fixtures/recall/sample.test.ts']), ['fixtures'])
  const steps = prePush(['fixtures/recall/sample.test.ts'])
  for (const gate of ['check:corpus', 'check:recall', 'check:oracle', 'check:next-duplicates']) assert.ok(steps.includes(`npm run ${gate}`), gate)
  assert.ok(steps.includes('cargo test --all-targets --all-features --locked'))
  assert.equal(steps.some(step => /^cargo (?:clippy|doc)/.test(step)), false, steps.join())
  assert.equal(steps.includes('npm run check:coverage:rust'), false)
})

test('the coverage gate script also runs Rust coverage', () => {
  assert.ok(prePush(['scripts/check/check-coverage.mjs']).includes('npm run check:coverage:rust'))
})

test('a developer tool runs its tests and no compile', () => {
  assert.deepEqual(hookScope(['scripts/dev/probe.mjs']), ['tooling'])
  const steps = prePush(['scripts/dev/probe.mjs'])
  assert.ok(steps.includes('npm run test:dev-checks'))
  assert.equal(steps.some(step => step.startsWith('cargo')), false)
})

test('Rust runs its lints, tests, corpus gates and coverage, not the packaging around the binary', () => {
  for (const path of ['src/model.rs', 'tests/example.rs', 'examples/depth.rs', 'Cargo.toml', 'Cargo.lock']) {
    assert.deepEqual(hookScope([path]), ['rust'])
    const steps = prePush([path])
    for (const step of ['cargo fmt --all --check', 'cargo clippy --all-targets --all-features --locked', 'cargo doc --no-deps --document-private-items --locked', 'cargo test --all-targets --all-features --locked', 'cargo build --release --locked', 'npm run check:corpus', 'npm run check:recall', 'npm run verify:titles', 'npm run check:oracle', 'npm run check:behavior-runtime', 'npm run check:hooks-differential', 'npm run check:next-duplicates', 'npm run check:duplication', 'npm run check:coverage:rust']) assert.ok(steps.includes(step), step)
    for (const step of ['npm test', 'npm run test:action', 'npm run pack:check', 'npm run check:coverage:js', 'npm run check']) assert.equal(steps.includes(step), false, step)
    assert.equal(hookCommands('pre-commit', [path]).some(step => /^cargo (?:clippy|test|build)/.test(step.join(' '))), false)
  }
})

test('a mixed change selects each surface once', () => {
  assert.deepEqual(hookScope(['README.md', 'src/model.rs', 'fixtures/a.test.ts']), ['docs', 'fixtures', 'rust'])
  const steps = prePush(['README.md', 'src/model.rs', 'fixtures/a.test.ts', '.github/workflows/ci.yml'])
  assert.equal(new Set(steps).size, steps.length)
  assert.ok(steps.includes('npm run check:coverage:rust') && steps.includes('npm run check:coverage:js'))
})

for (const path of ['package.json', 'package-lock.json', '.cargo/config.toml', 'new-script.sh', ' docs/guide.md', '\ndocs/guide.md', 'docs/README.md.rs', 'docs/data.json', 'src/data.json', 'build.rs']) {
  test(`unknown or shared scope stays full: ${JSON.stringify(path)}`, () => {
    assert.deepEqual(hookScope(['README.md', path]), ['full'])
    assert.deepEqual(hookCommands('pre-push', [path]), commands('full'))
  })
}

test('empty scope remains static; unknown scope remains full', () => {
  assert.deepEqual(hookScope([]), [])
  assert.deepEqual(hookScope(null), ['full'])
  assert.ok(hookCommands('pre-push', null).some(step => step.join(' ') === 'npm run check:coverage:rust'))
  assert.equal(hookCommands('pre-push', []).some(step => step[0] === 'cargo'), false)
})

for (const name of [' docs/guide.md', '\ndocs/guide.md', 'src/空 白.rs']) test(`Git path decoding retains exact bytes: ${JSON.stringify(name)}`, () => fixture(root => {
  mkdirSync(dirname(join(root, name)), { recursive: true })
  writeFileSync(join(root, name), 'content\n')
  git(root, 'add', '--', name)
  const [steps] = plannedChecks(root, 'pre-commit')
  assert.ok(steps.includes('npm run test:dev-checks'), 'unknown staged path cannot become docs-only')
  if (name.endsWith('.rs')) assert.ok(steps.includes('cargo fmt --all --check'))
}))

test('existing remote delta handles force updates and HEAD source refs without reading dirty HEAD', () => fixture(root => {
  const base = git(root, 'rev-parse', 'HEAD')
  const remote = commitChanges(root, { 'README.md': 'old docs\n' })
  git(root, 'checkout', '--quiet', '--detach', base)
  const proposed = commitChanges(root, { 'README.md': 'replacement docs\n' })
  commitChanges(root, { 'src/lib.rs': 'pub fn later() {}\n' })
  const [steps] = plannedChecks(root, 'pre-push', `HEAD ${proposed} refs/heads/topic ${remote}`)
  assert.ok(steps.includes('npm run check:docs'))
  assert.equal(steps.some(step => step.startsWith('cargo ')), false)
}))

for (const remote of [zero, oid]) test(`unavailable push baseline ${remote.slice(0, 4)} never narrows checks`, () => fixture(root => {
  const proposed = commitChanges(root, { 'README.md': '# docs\n' })
  const [steps] = plannedChecks(root, 'pre-push', `refs/heads/topic ${proposed} refs/heads/topic ${remote}`)
  assert.ok(steps.includes('npm run check:coverage:rust'))
}))

test('non-branch destinations retain full checks even for docs-only changes', () => fixture(root => {
  const before = git(root, 'rev-parse', 'HEAD')
  const proposed = commitChanges(root, { 'README.md': '# docs\n' })
  const [steps] = plannedChecks(root, 'pre-push', `refs/tags/v1 ${proposed} refs/tags/v1 ${before}`)
  assert.ok(steps.includes('npm run check:coverage:rust'))
}))

test('same proposed commit unions distinct remote baselines once and ignores staged changes', () => fixture(root => {
  const base = git(root, 'rev-parse', 'HEAD')
  const remote = commitChanges(root, { 'src/lib.rs': 'pub fn earlier() {}\n' })
  const proposed = commitChanges(root, { 'README.md': '# docs\n' })
  writeFileSync(join(root, 'source.txt'), 'unrelated staged change\n')
  git(root, 'add', 'source.txt')
  const [steps] = plannedChecks(root, 'pre-push', `refs/heads/topic ${proposed} refs/heads/a ${remote}\nrefs/heads/topic ${proposed} refs/heads/b ${base}`)
  assert.ok(steps.includes('npm run check:coverage:rust'))
  assert.equal(plannedChecks(root, 'pre-push', `refs/heads/topic ${proposed} refs/heads/a ${remote}\nrefs/heads/topic ${proposed} refs/heads/b ${remote}`).length, 1)
}))

for (const rename of [false, true]) test(`push scope retains removed Rust ${rename ? 'rename source' : 'file'}`, () => fixture(root => {
  const remote = commitChanges(root, { 'src/old.rs': 'pub fn old() {}\n' })
  if (rename) git(root, 'mv', 'src/old.rs', 'README.md')
  else git(root, 'rm', 'src/old.rs')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'remove Rust')
  const proposed = git(root, 'rev-parse', 'HEAD')
  const [steps] = plannedChecks(root, 'pre-push', `refs/heads/topic ${proposed} refs/heads/topic ${remote}`)
  assert.ok(steps.includes('npm run check:coverage:rust'))
}))

test('the shell hook forwards every remote baseline to a shared proposed driver', () => fixture(root => {
  const before = git(root, 'rev-parse', 'HEAD')
  const driver = "import { readFileSync } from 'node:fs'; console.error('REFS=' + readFileSync(0, 'utf8').trim().split('\\n').length)\n"
  const proposed = commitChanges(root, { 'scripts/dev/verify.mjs': driver })
  const input = `refs/heads/topic ${proposed} refs/heads/a ${before}\nrefs/heads/topic ${proposed} refs/heads/b ${zero}\n`
  const run = spawnSync('sh', [resolve('.githooks/pre-push')], { cwd: root, input, encoding: 'utf8' })
  assert.equal(run.status, 0, run.stderr)
  assert.equal(run.stderr.trim(), 'REFS=2')
}))

test('docs-only shell hooks execute static subprocesses, never Cargo, and propagate a selected failure', () => fixture(root => {
  const before = commitChanges(root, {
    'scripts/dev/verify.mjs': readFileSync(resolve('scripts/dev/verify.mjs'), 'utf8'),
    'scripts/check/check-docstrings.mjs': 'process.exit(0)\n',
    'README.md': '# Before\n',
  })
  git(root, 'update-ref', 'refs/remotes/origin/main', before)
  writeFileSync(join(root, 'README.md'), '# After\n')
  git(root, 'add', 'README.md')
  withGitleaks(0, env => {
    const bin = env.PATH.split(delimiter)[0]
    const log = join(root, 'commands.log')
    writeFileSync(join(bin, 'npm'), '#!/bin/sh\nprintf "%s\\n" "$*" >> "$CHECK_LOG"\nif [ "$2" = check:docs ] && [ "${FAIL_DOCS:-0}" = 1 ]; then exit 7; fi\n', { mode: 0o755 })
    writeFileSync(join(bin, 'cargo'), '#!/bin/sh\necho forbidden-cargo >> "$CHECK_LOG"\nexit 99\n', { mode: 0o755 })
    env = { ...env, CHECK_LOG: log }
    const commit = spawnSync('sh', [resolve('.githooks/pre-commit')], { cwd: root, env, encoding: 'utf8' })
    assert.equal(commit.status, 0, commit.stderr)
    const proposed = commitChanges(root, { 'README.md': '# After\n' })
    const input = `refs/heads/topic ${proposed} refs/heads/topic ${before}\n`
    const push = spawnSync('sh', [resolve('.githooks/pre-push')], { cwd: root, env, input, encoding: 'utf8' })
    assert.equal(push.status, 0, push.stderr)
    const expected = ['check:action-pins', 'check:release-workflow', 'check:versions', 'check:docs', 'check:names', 'skills:check'].map(gate => 'run ' + gate)
    assert.deepEqual(readFileSync(log, 'utf8').trim().split('\n'), [...expected, ...expected])
    const failed = spawnSync('sh', [resolve('.githooks/pre-push')], { cwd: root, env: { ...env, FAIL_DOCS: '1' }, input, encoding: 'utf8' })
    assert.notEqual(failed.status, 0)
    assert.match(failed.stderr, /npm run check:docs failed/)
    assert.doesNotMatch(readFileSync(log, 'utf8'), /forbidden-cargo/)
  })
}))

test('a failed ref-grouping command cannot become an empty successful push check', () => fixture(root => {
  const before = git(root, 'rev-parse', 'HEAD')
  const proposed = commitChanges(root, { 'scripts/dev/verify.mjs': "console.error('driver should not run')\n" })
  withGitleaks(0, env => {
    writeFileSync(join(env.PATH.split(delimiter)[0], 'awk'), '#!/bin/sh\nexit 42\n', { mode: 0o755 })
    const input = `refs/heads/topic ${proposed} refs/heads/topic ${before}\n`
    const run = spawnSync('sh', [resolve('.githooks/pre-push')], { cwd: root, env, input, encoding: 'utf8' })
    assert.notEqual(run.status, 0)
    assert.doesNotMatch(run.stderr, /driver should not run/)
  })
}))
