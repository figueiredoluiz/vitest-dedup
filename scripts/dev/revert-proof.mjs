#!/usr/bin/env node
// Proves each fix fails its checks when it is undone, and leaves the tree as it found it.
//
// A spec names checks and mutants:
//   { "build": ["cargo", "build", "--locked"],
//     "checks": [{ "name": "unit", "command": ["cargo", "test", "--lib", "some_test"] }],
//     "mutants": [{ "name": "guard off", "file": "src/a.rs", "find": "old", "replace": "new",
//                   "expect": ["unit"] }] }
// The checks must pass untouched first. A mutant must match `find` exactly once and must make
// every check in `expect` fail, or at least one check when it names none. `build` is required: it
// separates a mutant that compiled and failed from one that never ran. For JavaScript, `node
// --check <file>` is enough. A `cargo test` check is also compiled by itself with `--no-run` before
// it runs, under the same flags, so a test that does not compile never reads as caught. An
// interrupted run restores every file before it exits.
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, realpathSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { requireMacOS, runCommand } from './proof-command.mjs'

/** The digest a restored file is compared by. */
const digest = (text) => createHash('sha256').update(text).digest('hex')

/**
 * The command that compiles what `command` runs, when compiling and running are separate: the same
 * `cargo test`, flags and all, with `--no-run`.
 * @param {string[]} command A check's command.
 * @returns {string[]|null} The compile step, or null when the check has none.
 */
export function compileStep(command) {
  const [tool] = command
  if (!/^cargo(?:\.exe)?$/i.test(tool.split(/[\\/]/).at(-1))) return null
  let index = command[1]?.startsWith('+') ? 2 : 1
  for (; index < command.length; index++) {
    const arg = command[index]
    if (!arg.startsWith('-')) break
    if (['--color', '--config', '-C', '-Z'].includes(arg)) index++
    else if (!/^(?:-v+|-q|--verbose|--quiet|--locked|--offline|--frozen|--color=.+|--config=.+|-C.+|-Z.+)$/.test(arg)) {
      throw new Error(`cannot determine Cargo command after ${arg}; use a direct cargo test command`)
    }
  }
  return ['test', 't'].includes(command[index])
    ? [...command.slice(0, index + 1), '--no-run', ...command.slice(index + 1)]
    : null
}

/**
 * Why `spec` cannot run, if it cannot.
 * @param {object} spec A parsed spec.
 * @returns {string[]} Problems; empty when it can.
 */
export function problems(spec) {
  const out = []
  const command = (value) => Array.isArray(value) && value.length > 0 && value.every((part) => typeof part === 'string')
  if (!Array.isArray(spec?.checks) || spec.checks.length === 0) return ['checks must list at least one check']
  if (!Array.isArray(spec.mutants) || spec.mutants.length === 0) return ['mutants must list at least one mutant']
  if (!command(spec.build)) out.push('build must be a command')
  const names = new Set()
  for (const check of spec.checks) {
    if (typeof check?.name !== 'string' || !command(check.command)) out.push('every check needs a name and a command')
    else if (names.has(check.name)) out.push(`check ${check.name}: named twice`)
    else names.add(check.name)
  }
  for (const mutant of spec.mutants) {
    for (const key of ['name', 'file', 'find']) if (typeof mutant?.[key] !== 'string' || mutant[key] === '') out.push(`mutant ${mutant?.name}: ${key} is required`)
    if (typeof mutant?.replace !== 'string') out.push(`mutant ${mutant?.name}: replace is required`)
    else if (mutant.replace === mutant.find) out.push(`mutant ${mutant.name}: replace equals find, so nothing changes`)
    for (const name of mutant?.expect ?? []) if (!names.has(name)) out.push(`mutant ${mutant.name}: expects unknown check ${name}`)
  }
  return out
}

/**
 * Apply one edit to `text`.
 * @param {string} text The file.
 * @param {{name: string, find: string, replace: string}} mutant The edit.
 * @returns {string} The edited file.
 * @throws {Error} Unless `find` occurs exactly once: an edit that did not apply proves nothing.
 */
export function mutate(text, { name, find, replace }) {
  const count = text.split(find).length - 1
  if (count !== 1) throw new Error(`mutant ${name}: find matches ${count} times, expected 1`)
  return text.replace(find, () => replace)
}

/**
 * Whether a mutant was caught.
 * @param {{expect?: string[]}} mutant The mutant.
 * @param {string[]} failed The checks that failed under it.
 * @returns {boolean} Whether every expected check failed, or any check when none is expected.
 */
export function caught({ expect = [] }, failed) {
  return expect.length ? expect.every((name) => failed.includes(name)) : failed.length > 0
}

/**
 * Run the proof. Each file is restored in `finally`, and verified by digest afterwards.
 * @param {object} spec The spec.
 * @param {{run: (command: string[]) => Promise<boolean>, read: (file: string) => string, write: (file: string, text: string) => void, print: (line: string) => void}} io Adapters.
 * @returns {Promise<{name: string, verdict: 'caught'|'survived'|'not-built', failed: string[]}[]>} One result per mutant.
 * @throws {Error} When the spec is invalid, an edit does not apply, a check fails untouched, or a file is not restored.
 */
export async function prove(spec, { run, read, write, print }) {
  const invalid = problems(spec)
  if (invalid.length) throw new Error(invalid.join('\n'))
  // Every edit is applied in memory first, so a stale anchor fails before anything is built.
  const originals = new Map(spec.mutants.map(({ file }) => [file, read(file)]))
  for (const mutant of spec.mutants) mutate(originals.get(mutant.file), mutant)
  const steps = [spec.build, ...spec.checks.map((check) => compileStep(check.command)).filter(Boolean)]
  const build = async () => {
    for (const step of steps) if (!(await run(step))) return false
    return true
  }
  const failing = async () => {
    const failed = []
    for (const check of spec.checks) if (!(await run(check.command))) failed.push(check.name)
    return failed
  }
  if (!(await build())) throw new Error('the untouched tree does not build')
  const baseline = await failing()
  if (baseline.length) throw new Error(`checks fail on the untouched tree: ${baseline.join(', ')}`)
  const results = []
  for (const mutant of spec.mutants) {
    const original = originals.get(mutant.file)
    let result
    try {
      write(mutant.file, mutate(original, mutant))
      if (await build()) {
        const failed = await failing()
        result = { name: mutant.name, verdict: caught(mutant, failed) ? 'caught' : 'survived', failed }
      } else result = { name: mutant.name, verdict: 'not-built', failed: [] }
    } finally {
      write(mutant.file, original)
    }
    print(`${result.verdict.toUpperCase()} ${mutant.name}${result.failed.length ? ` (${result.failed.join(', ')})` : ''}`)
    results.push(result)
  }
  for (const [file, original] of originals) {
    if (digest(read(file)) !== digest(original)) throw new Error(`${file} was not restored`)
  }
  if (!(await build())) throw new Error('the restored tree does not build, so its artifacts are the last mutant')
  return results
}

/**
 * The command line: real processes and files, with every file restored on an interrupt. A check
 * runs asynchronously so a signal reaches this handler while it is running.
 * @param {string[]} args Arguments after the script.
 * @returns {Promise<boolean>} Whether every mutant was caught.
 * @throws {Error} On bad usage or a failed proof.
 */
async function cli(args) {
  requireMacOS()
  const [path, ...rest] = args
  if (!path || rest.length) throw new Error('usage: revert-proof.mjs <spec.json>')
  const touched = new Map()
  const controller = new AbortController()
  const handlers = new Map()
  let interrupted = null
  for (const [signal, code] of [['SIGINT', 130], ['SIGTERM', 143], ['SIGHUP', 129]]) {
    const handler = () => {
      if (interrupted !== null) return
      interrupted = code
      controller.abort(new Error(`interrupted by ${signal}`))
    }
    handlers.set(signal, handler)
    process.on(signal, handler)
  }
  try {
    const results = await prove(JSON.parse(readFileSync(path, 'utf8')), {
      run: (command) => runCommand(command, { signal: controller.signal }),
      read: (file) => readFileSync(file, 'utf8'),
      write: (file, text) => {
        if (!touched.has(file)) touched.set(file, readFileSync(file, 'utf8'))
        writeFileSync(file, text)
      },
      print: console.log,
    })
    return results.every((result) => result.verdict === 'caught')
  } catch (error) {
    if (interrupted !== null) error.exitCode = interrupted
    throw error
  } finally {
    // runCommand settles only after stopping the owned command tree, so no descendant can
    // race this restoration. Keep signal handlers installed until the restoration is complete.
    for (const [file, text] of touched) writeFileSync(file, text)
    for (const [signal, handler] of handlers) process.removeListener(signal, handler)
  }
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  cli(process.argv.slice(2)).then(
    (proved) => {
      process.exitCode = proved ? 0 : 1
    },
    (error) => {
      console.error(error.message)
      process.exitCode = error.exitCode ?? 1
    },
  )
}
