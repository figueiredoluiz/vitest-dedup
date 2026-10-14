import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { test } from 'node:test'
import { changedLines, cli, coverage, functionsIn, main, scopeDiff } from './check-docstrings.mjs'

/** Supply an owned source checkout and always remove its temporary files. */
function fixture(body) {
  const root = mkdtempSync(join(tmpdir(), 'docstrings-test-'))
  try { return body(root) } finally { rmSync(root, { recursive: true, force: true }) }
}

/** Describe an entire new file using the same zero-context hunks Git emits. */
function added(path, source) {
  return `diff --git a/${path} b/${path}\n--- /dev/null\n+++ b/${path}\n@@ -0,0 +1,${source.split('\n').length} @@\n`
}

test('the 80 percent boundary passes and a real missing-doc regression fails', () => fixture(root => {
  const source = Array.from({ length: 5 }, (_, i) => `${i < 4 ? '/** Return the selected case value. */\n' : ''}function case${i}() { return ${i} }`).join('\n')
  writeFileSync(join(root, 'case.ts'), source)
  const diff = added('case.ts', source)
  const pass = coverage(root, diff)
  assert.equal(pass.functions.length, 5)
  assert.equal(pass.documented, 4)
  assert.equal(pass.percent, 80)
  assert.equal(pass.passed, true)
  writeFileSync(join(root, 'case.ts'), source.replace('/** Return the selected case value. */', ''))
  const fail = coverage(root, diff)
  assert.equal(fail.functions.length, 5)
  assert.equal(fail.documented, 3)
  assert.equal(fail.passed, false)
  writeFileSync(join(root, 'scope.diff'), diff)
  assert.throws(() => main(['--diff-file', join(root, 'scope.diff')], root), /below/)
  // Exercise the CLI catch in this process; its coverage does not depend on child instrumentation.
  assert.equal(cli(['--diff-file', join(root, 'scope.diff')], root), 1)
  writeFileSync(join(root, 'empty.diff'), '')
  assert.equal(cli(['--diff-file', join(root, 'empty.diff')], root), 0)
  const child = spawnSync(process.execPath, [resolve('scripts/check/check-docstrings.mjs'), '--diff-file', join(root, 'scope.diff')], { cwd: root, encoding: 'utf8' })
  assert.equal(child.status, 1)
  assert.match(child.stdout, /3\/5/)
}))

test('Rust strings and ordinary comments do not invent functions or documentation', () => {
  const source = `const TEXT: &str = r#"fn fake() {}"#;
// fn fake_comment() {}
/// Return the real value.
#[cfg(test)]
fn real() { let text = "function fake_js() {}"; }
// This is an ordinary comment.
fn missing() {}
//// This is also an ordinary Rust comment.
fn four_slashes() {}
`
  assert.deepEqual(functionsIn('case.rs', source).map(fn => [fn.name, fn.documented]), [['real', true], ['missing', false], ['four_slashes', false]])
})

test('JS strings and templates are opaque while nested named declarations are counted', () => {
  const source = `const text = 'function fake() {}';
const template = \`function fake_template() {}\`;
/** Run the outer operation. */
export async function outer({ value = {} }) {
  /** Return one generated value. */
  function* inner() { yield value }
}
`
  assert.deepEqual(functionsIn('case.ts', source).map(fn => [fn.name, fn.documented]), [['outer', true], ['inner', true]])
  assert.equal(functionsIn('case.js', '/** */\nfunction emptyDoc() {}')[0].documented, false)
})

test('unchanged undocumented functions are excluded but body edits and deleted docs count', () => fixture(root => {
  const source = 'fn old() {}\n/// Perform the current work.\nfn changed() { work(); }\n'
  writeFileSync(join(root, 'case.rs'), source)
  const report = coverage(root, 'diff --git a/case.rs b/case.rs\n+++ b/case.rs\n@@ -3 +3 @@\n')
  assert.deepEqual(report.functions.map(fn => fn.name), ['changed'])
  assert.equal(report.passed, true)
  writeFileSync(join(root, 'case.rs'), 'fn old() {}\nfn changed() { work(); }\n')
  const deleted = coverage(root, 'diff --git a/case.rs b/case.rs\n+++ b/case.rs\n@@ -2 +1,0 @@\n')
  assert.equal(deleted.functions.some(fn => fn.name === 'changed' && !fn.documented), true)
  assert.equal(deleted.passed, false)
}))

test('deleted files and non-source changes are N/A; malformed hunks and paths are refused', () => fixture(root => {
  assert.equal(coverage(root, 'diff --git a/a.rs b/a.rs\n+++ /dev/null\n').functions.length, 0)
  assert.equal(coverage(root, added('README.md', 'text')).functions.length, 0)
  assert.throws(() => changedLines('@@ -1 +1 @@'), /Malformed/)
  assert.throws(() => changedLines('not a diff'), /Malformed/)
  assert.throws(() => changedLines('diff --git a/a.rs b/a.rs\n+++ b/../secret.rs'), /Unsupported/)
  assert.throws(() => changedLines('diff --git a/a.rs b/a.rs\n+++ "b/odd.rs"'), /Unsupported/)
  assert.throws(() => functionsIn('case.js', 'function broken() {'))
  assert.throws(() => functionsIn('case.rs', 'fn broken() {'), /Unbalanced/)
  assert.throws(() => main(['--threshold', '0'], root), /Usage/)
}))

test('regex literals, JSX and TS object return types cannot hide a touched function body', () => fixture(root => {
  const source = 'function typed(): { value: string } {\n  const pattern = /}/;\n  return { value: "ok" };\n}\n'
  writeFileSync(join(root, 'case.ts'), source)
  const report = coverage(root, 'diff --git a/case.ts b/case.ts\n+++ b/case.ts\n@@ -3 +3 @@\n')
  assert.equal(report.functions.length, 1)
  assert.equal(report.passed, false)
  const jsx = '/** Return the view. */\nfunction view() { return <div>{"}"}</div> }'
  assert.equal(functionsIn('case.tsx', jsx)[0].documented, true)
  const inline = '/** Return one. */ export function first() { return 1 }'
  assert.equal(functionsIn('case.js', inline)[0].documented, true)
}))

test('removed docs remain in scope across multiline Rust attributes', () => fixture(root => {
  const source = '#[cfg(any(\n test, feature = "enabled"\n))]\nfn missing() {}\n'
  writeFileSync(join(root, 'case.rs'), source)
  const report = coverage(root, 'diff --git a/case.rs b/case.rs\n+++ b/case.rs\n@@ -1 +0,0 @@\n')
  assert.equal(report.functions.length, 1)
  assert.equal(report.passed, false)
}))

test('named expressions use their direct variable docs; unrelated docs cannot cover them', () => {
  const source = '/** Return one. */\nconst x = function named() { return 1 };\nconst y = function missing() { return 2 };'
  assert.deepEqual(functionsIn('case.js', source).map(fn => [fn.name, fn.documented]), [['named', true], ['missing', false]])
  const inline = 'const x = /** Return one. */ function named() { return 1 };'
  assert.equal(functionsIn('case.js', inline)[0].documented, true)
  const declared = '/** Declare the callback. */\ndeclare function named(): void;'
  assert.equal(functionsIn('case.ts', declared)[0].documented, true)
  const unrelated = '/** Build the callback holder. */\nfunction outer() { return function named() {} }'
  assert.equal(functionsIn('case.js', unrelated)[1].documented, false)
})

test('real Git scope includes staged changes and proposed commits without a remote base', () => fixture(root => {
  /** Execute an isolated fixture's Git command with no inherited repository paths. */
  function git(...args) {
    return execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }).trim()
  }
  git('init', '--quiet')
  writeFileSync(join(root, 'case.rs'), 'fn missing() {}\n')
  git('add', 'case.rs')
  assert.equal(coverage(root, scopeDiff(root, { staged: true })).passed, false)
  git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'fixture')
  assert.equal(coverage(root, scopeDiff(root, { revision: 'HEAD' })).passed, false)
  assert.equal(scopeDiff(root), '')
}))
