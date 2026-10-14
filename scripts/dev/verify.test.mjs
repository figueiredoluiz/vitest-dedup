import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { existsSync, lstatSync, mkdirSync, mkdtempSync, realpathSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, delimiter, join, resolve } from 'node:path'
import { test } from 'node:test'
import { cleanEnvironment, commands, installHooks, main, pushCommits, runSteps, rustChanged, withLock, withNodeModules, withSnapshot, withTargetCache } from './verify.mjs'
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

test('fast checks leave corpus source alone and skip Cargo for non-Rust staged changes', () => {
  for (const profile of ['fast', 'full']) assert.deepEqual(commands(profile)[0], ['node', 'scripts/check/check-docstrings.mjs'])
  assert.equal(rustChanged(['fixtures/bad.test.ts']), false)
  for (const path of ['src/model.rs', 'Cargo.lock', 'Cargo.toml', 'scripts/check/check-coverage.mjs']) assert.equal(rustChanged([path]), true)
  assert.equal(commands('fast', false).some(step => step[0] === 'cargo'), false)
  const full = commands('full')
  assert.ok(full.some(step => step.join(' ') === 'cargo test --all-targets --all-features --locked'))
  assert.ok(full.some(step => step.join(' ') === 'npm run check:coverage:rust'))
  assert.equal(commands('full', false).some(step => step.includes('check:coverage:rust')), false)
  assert.throws(() => commands('unknown'), /Unknown/)
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

test('lock rejects concurrent work and is removed on success and failure', () => fixture(root => {
  const common = join(root, '.git')
  withLock(common, () => {
    assert.match(readFileSync(join(common, 'delivery-check.lock/owner'), 'utf8'), /^pid=/)
    assert.throws(() => withLock(common, () => assert.fail()), /already running/)
  })
  assert.equal(existsSync(join(common, 'delivery-check.lock')), false)
  assert.throws(() => withLock(common, () => { throw new Error('failed') }), /failed/)
  assert.equal(existsSync(join(common, 'delivery-check.lock')), false)
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
  const check = (snapshot, steps, options) => {
    snapshots.push(snapshot)
    assert.equal(options.env.CARGO_TARGET_DIR, join(root, 'target'))
    assert.equal(options.env.CARGO_LLVM_COV_TARGET_DIR, join(root, 'target/llvm-cov-target'))
    assert.ok(steps.some(step => step[1] === 'clippy'))
  }
  writeFileSync(join(root, 'test.rs'), 'fn main() {}\n'); git(root, 'add', 'test.rs')
  main(['pre-commit'], { root, env, check, print: line => output.push(line) })
  const revision = git(root, 'rev-parse', 'HEAD')
  main(['pre-push', 'origin', 'url'], { root, env, input: `ref ${revision} remote ${zero}`, check, print: line => output.push(line) })
  assert.equal(snapshots.length, 2)
  assert.ok(snapshots.every(p => !existsSync(p)))
  main(['pre-push'], { root, env, input: `ref ${zero} remote ${revision}`, check: () => assert.fail(), print: line => output.push(line) })
  assert.match(output.at(-1), /No commits/)
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


test('deleting staged Rust still runs Clippy', () => fixture(root => {
  writeFileSync(join(root, 'removed.rs'), 'fn main() {}\n')
  git(root, 'add', 'removed.rs')
  git(root, '-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '-m', 'Rust fixture')
  git(root, 'rm', 'removed.rs')
  main(['pre-commit'], { root, print: () => {}, check: (snapshot, steps) => {
    assert.equal(existsSync(join(snapshot, 'removed.rs')), false)
    assert.ok(steps.some(step => step[1] === 'clippy'))
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
      spawn: (_cmd, _args, options) => { assert.equal(options.env.RUSTFLAGS, expected); return { status: 0 } },
    })
  }
}))
