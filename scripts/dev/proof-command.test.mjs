import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { requireMacOS, runCommand, stopTree } from './proof-command.mjs'
import { prove } from './revert-proof.mjs'

/** A real child script, with its path retained for the test and removed afterward. */
async function script(source, body) {
  const work = mkdtempSync(join(tmpdir(), 'proof-command-test-'))
  const path = join(work, 'check.mjs')
  writeFileSync(path, source)
  try { await body([process.execPath, path], work) }
  finally { rmSync(work, { recursive: true, force: true }) }
}

test('completed exit codes remain usable, with failed-check output retained', { skip: process.platform !== 'darwin' }, async () => {
  await script('console.log("an assertion failed"); process.exit(Number(process.argv[2]));', async (command) => {
    assert.equal(await runCommand([...command, '0']), true)
    const diagnostics = []
    assert.equal(await runCommand([...command, '1'], { print: (line) => diagnostics.push(line) }), false)
    assert.equal(diagnostics.length, 1)
    assert.match(readFileSync(diagnostics[0].split('; log: ')[1], 'utf8'), /an assertion failed/)
  })
})

test('a completed command cleans up inherited output pipes before the deadline', { skip: process.platform !== 'darwin', timeout: 10_000 }, async () => {
  await script(`import {spawn} from 'node:child_process'; spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], {stdio:'inherit'}).unref(); console.log('completed');`, async (command) => {
    let result
    await assert.doesNotReject(async () => { result = await runCommand(command, { timeoutMs: 1_500 }) })
    assert.equal(result, true)
  })
})

test('signals, spawn failures, deadlines and output exhaustion reject commands', { skip: process.platform !== 'darwin', timeout: 15_000 }, async () => {
  await assert.rejects(runCommand([join(tmpdir(), 'no-such-proof-executable.exe')]), /ENOENT|terminated|log:/)
  await script('process.kill(process.pid, "SIGTERM");', async (command) => {
    await assert.rejects(runCommand(command), /terminated by SIGTERM/)
  })
  await script('setInterval(()=>{}, 1000)', async (command) => {
    await assert.rejects(runCommand(command, { timeoutMs: 100 }), /timed out/)
  })
  await script('console.log("x".repeat(20000))', async (command) => {
    await assert.rejects(runCommand(command, { maxOutputBytes: 1024 }), (error) => {
      assert.match(error.message, /output exceeded/)
      assert.equal(statSync(error.message.split('; log: ')[1]).size, 1024)
      return true
    })
  })
})

test('an operational failure cannot catch a mutant, and restores its file', { skip: process.platform !== 'darwin' }, async () => {
  await script(`import {readFileSync} from 'node:fs'; if(readFileSync(process.argv[2],'utf8')==='bad') process.kill(process.pid,'SIGTERM');`, async (command, work) => {
    const target = join(work, 'target.txt')
    writeFileSync(target, 'good')
    const printed = []
    const spec = { build: [process.execPath, '--check', command[1]], checks: [{ name: 'assertions', command: [...command, target] }], mutants: [{ name: 'm', file: target, find: 'good', replace: 'bad' }] }
    await assert.rejects(prove(spec, { run: runCommand, read: (file) => readFileSync(file, 'utf8'), write: writeFileSync, print: (line) => printed.push(line) }), /terminated by SIGTERM/)
    assert.deepEqual(printed, [])
    assert.equal(readFileSync(target, 'utf8'), 'good')
  })
})

test('cancellation waits for tree termination and refuses later commands', { skip: process.platform !== 'darwin' }, async () => {
  const controller = new AbortController()
  controller.abort(new Error('already interrupted'))
  await assert.rejects(runCommand([process.execPath], { signal: controller.signal }), /already interrupted/)
  await script('setInterval(()=>{}, 1000)', async (command) => {
    const controller = new AbortController()
    const running = runCommand(command, { signal: controller.signal })
    const timer = setTimeout(() => controller.abort(new Error('cancelled')), 100)
    try { await assert.rejects(running, /cancelled/) }
    finally { clearTimeout(timer) }
  })
})

test('tree termination uses owned process groups and propagates cleanup failures', async () => {
  const child = { pid: 4321 }
  const killed = []
  await stopTree(child, { kill: (...args) => killed.push(args) })
  assert.deepEqual(killed, [[-4321, 'SIGKILL']])
  await stopTree({})
  await stopTree(child, { kill: () => { throw Object.assign(new Error(), { code: 'ESRCH' }) } })
  await assert.rejects(stopTree(child, { kill: () => { throw new Error('denied') } }), /denied/)
})

test('the shared platform guard accepts macOS and refuses Windows and Linux commands', async () => {
  const platform = Object.getOwnPropertyDescriptor(process, 'platform')
  try {
    Object.defineProperty(process, 'platform', { value: 'darwin' })
    assert.doesNotThrow(requireMacOS)
    for (const value of ['win32', 'linux']) {
      Object.defineProperty(process, 'platform', { value })
      assert.throws(requireMacOS, /supported only on macOS/)
      await assert.rejects(runCommand(['must-not-start']), /supported only on macOS/)
    }
  } finally { Object.defineProperty(process, 'platform', platform) }
})

test('unsupported CLI platforms refuse before reading specs or mutating files', () => {
  const work = mkdtempSync(join(tmpdir(), 'proof-platform-'))
  const tool = join(dirname(fileURLToPath(import.meta.url)), 'revert-proof.mjs')
  const source = join(work, 'source.txt')
  const marker = join(work, 'command-ran')
  const check = join(work, 'check.mjs')
  const driver = join(work, 'driver.mjs')
  const spec = join(work, 'spec.json')
  writeFileSync(source, 'good')
  writeFileSync(check, "import {writeFileSync} from 'node:fs'; writeFileSync(process.argv[2], 'ran');")
  writeFileSync(driver, "import {pathToFileURL} from 'node:url'; const [platform, tool, spec] = process.argv.slice(2); Object.defineProperty(process, 'platform', {value:platform}); process.argv = [process.execPath, tool, spec]; await import(pathToFileURL(tool).href);")
  writeFileSync(spec, JSON.stringify({ build: [process.execPath, check, marker], checks: [{name:'check',command:[process.execPath, check, marker]}], mutants: [{name:'m',file:source,find:'good',replace:'bad'}] }))
  try {
    for (const platform of ['win32', 'linux']) {
      for (const input of [spec, join(work, 'missing.json')]) {
        const result = spawnSync(process.execPath, [driver, platform, tool, input], { encoding: 'utf8', timeout: 10_000 })
        assert.equal(result.error, undefined)
        assert.equal(result.status, 1)
        assert.match(result.stderr, /supported only on macOS/)
        assert.doesNotMatch(result.stderr, /ENOENT/)
        assert.equal(result.stdout, '')
        assert.equal(readFileSync(source, 'utf8'), 'good')
        assert.equal(existsSync(marker), false)
      }
    }
  } finally { rmSync(work, { recursive: true, force: true }) }
})
