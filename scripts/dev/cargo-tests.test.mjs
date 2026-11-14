import assert from 'node:assert/strict'
import { test } from 'node:test'
import { main, moduleCoverage, plan, testsRun } from './cargo-tests.mjs'

const LISTING = 'lang::tests::reads: test\nlang::js_text::tests::lone: test\nrules::tests::a: test\nmodel_free: test\n\n4 tests, 0 benchmarks\n'

test('tests run are summed across binaries; ignored and filtered tests are not', () => {
  assert.equal(testsRun('test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 10 filtered out; finished in 0.1s\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out\n'), 3)
  assert.equal(testsRun('test result: FAILED. 2 passed; 1 failed; 0 ignored\n'), 3)
  assert.equal(testsRun('test result: ok. 0 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out\n'), 0)
  assert.equal(testsRun(''), 0)
})

test('a module counts as covered only by a test under its own path', () => {
  assert.deepEqual(moduleCoverage(LISTING, ['lang', 'rules', 'model', 'js_text']), { covered: ['lang', 'rules'], uncovered: ['model', 'js_text'] })
})

test('library modules are filtered by path; one without tests widens to the whole library', () => {
  let listed = 0
  const list = () => { listed++; return LISTING }
  assert.deepEqual(plan(['lib', 'lang', 'rules'], list), { runs: [['test', '--lib', '--locked', '--', 'lang::', 'rules::']], notes: [] })
  const widened = plan(['lib', 'lang', 'model'], list)
  assert.deepEqual(widened.runs, [['test', '--lib', '--locked']])
  assert.match(widened.notes[0], /model::/)
  assert.equal(listed, 2)
  assert.deepEqual(plan(['lib'], () => assert.fail('no listing without modules')).runs, [['test', '--lib', '--locked']])
})

test('the binary is built before its tests, and integration targets are named', () => {
  assert.deepEqual(plan(['bin'], () => '').runs, [['build', '--locked'], ['test', '--bin', 'vitest-dedup', '--locked']])
  assert.deepEqual(plan(['test', 'cli_ux', 'sarif_paths'], () => '').runs, [['test', '--locked', '--test', 'cli_ux', '--test', 'sarif_paths']])
  assert.throws(() => plan(['test'], () => ''), /at least one target/)
  assert.throws(() => plan(['all'], () => ''), /Usage/)
})

/** Run main against scripted Cargo results, recording each invocation. */
function run(args, results) {
  const calls = []
  const sink = { text: '', write(chunk) { this.text += chunk } }
  const err = { text: '', write(chunk) { this.text += chunk } }
  const status = main(args, { spawn: (command, cargoArgs) => { calls.push([command, ...cargoArgs].join(' ')); return results.shift() }, out: sink, err })
  return { status, calls, out: sink.text, err: err.text }
}

test('a run where no test executed fails, though Cargo exited 0', () => {
  const result = run(['test', 'cli_ux'], [{ status: 0, stdout: 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n', stderr: '' }])
  assert.equal(result.status, 1)
  assert.match(result.err, /No test ran/)
})

test('a passing run reports its count, and a failing Cargo keeps its exit status and stops', () => {
  const pass = run(['bin'], [{ status: 0, stdout: '', stderr: 'Finished\n' }, { status: 0, stdout: 'test result: ok. 7 passed; 0 failed;\n', stderr: '' }])
  assert.equal(pass.status, 0)
  assert.deepEqual(pass.calls, ['cargo build --locked', 'cargo test --bin vitest-dedup --locked'])
  assert.match(pass.out, /7 tests ran/)
  const fail = run(['bin'], [{ status: 101, stdout: '', stderr: 'error[E0425]\n' }])
  assert.equal(fail.status, 101)
  assert.deepEqual(fail.calls, ['cargo build --locked'])
  assert.equal(run(['bin'], [{ error: new Error('spawn cargo ENOENT'), status: null }]).status, 1)
})

test('a failing library listing stops the run', () => {
  assert.throws(() => run(['lib', 'lang'], [{ status: 101, stdout: '', stderr: 'error\n' }]), /Listing the library tests failed/)
})
