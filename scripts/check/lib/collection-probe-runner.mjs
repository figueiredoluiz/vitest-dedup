// A Vitest runner that reports what Vitest collected, for `check-collection-oracle.mjs`.
//
// Loaded through `test.runner` under `vitest list --staticParse=false`: Vitest imports each test
// file and runs its describe callbacks, but runs no test or hook body. Plain `vitest list` parses
// files statically in Vitest 5 and never loads a runner, so it cannot be the oracle.
//
// One JSON line per collected file is appended to `$VITEST_DEDUP_ORACLE_OUT`:
//   {file, project, tests: [{title, mode}], tasks: [{kind, line}], registrations: [{kind, file, line}],
//    suiteHooks: {kind: n}, stubReads: ['specifier#export'], collectErrors: [message]}
// `tasks` is every collected test and suite with the line Vitest places it on in the test file
// (`includeTaskLocation`), `null` when no frame of the test file registered it.
// `registrations` is every hook registration in source order, read from the `STACK_TRACE_ERROR`
// each hook function creates (only `beforeAll`/`beforeEach` keep it reachable on the hook, so the
// error is observed at construction instead). `suiteHooks` counts the same hooks from the
// collected suites (`TestRunner.getSuiteHooks`); the gate requires the two to agree, so a probe
// that missed a registration fails instead of reporting fewer hooks. `stubReads` are the exports of
// missing modules (`collection-oracle-stubs.mjs`) the file read while it was collected.
import { appendFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { TestRunner } from 'vitest'

const OUT = process.env.VITEST_DEDUP_ORACLE_OUT
const KINDS = ['beforeAll', 'afterAll', 'beforeEach', 'afterEach', 'aroundAll', 'aroundEach']
// `beforeEach` called directly, or as `test.beforeEach`/`it.beforeEach` (V8 prints `Function.beforeEach`).
const HOOK_FRAME = new RegExp(`^\\s*at (?:[\\w$]+\\.)*(${KINDS.join('|')}) `)
const POSITION = /:(\d+):(\d+)\)?$/

/** A stack frame's file, as a path, whether V8 printed a path or a `file://` URL. */
function framePath(frame) {
  const location = frame.replace(/^\s*at (?:.* \()?/, '').replace(POSITION, '')
  return location.startsWith('file://') ? fileURLToPath(location) : location
}

// This runner's own frames (the `Error` proxy) sit above every registration.
const PROBE = fileURLToPath(import.meta.url)
/** Whether a stack frame belongs to Vitest itself or to Node, rather than to code under test. */
const isRuntimeFrame = (path) => /[\\/]node_modules[\\/](?:@vitest|vitest)[\\/]/.test(path) || !/^[\\/]|^[A-Za-z]:/.test(path)

/**
 * The hook kind and the site that declared it, from a registration error's stack: the frame of the
 * hook function itself, then the first frame below it outside Vitest. That is where the analyzer
 * places a hook too, so a hook declared in a helper module is named by the helper's line, not by
 * the test file's call of the helper. `null` when the error was not created by a hook function:
 * Vitest also creates one for each fixture and test, and a test registered by code a hook call runs
 * (a timeout's `valueOf`) has the hook's frame below its own code's.
 */
function registration(stack) {
  const frames = String(stack).split('\n')
  const hook = frames.findIndex((frame) => HOOK_FRAME.test(frame))
  if (hook < 0) return null
  const ownCode = (frame) => POSITION.test(frame) && !isRuntimeFrame(framePath(frame)) && framePath(frame) !== PROBE
  if (frames.slice(1, hook).some(ownCode)) return null
  const kind = frames[hook].match(HOOK_FRAME)[1]
  const site = frames.slice(hook + 1).find((frame) => POSITION.test(frame) && !isRuntimeFrame(framePath(frame)))
  if (!site) return { kind, file: null, line: null }
  return { kind, file: framePath(site), line: Number(site.match(POSITION)[1]) }
}

/**
 * Replaces the global `Error` with a proxy that hands each registration error to `observe`, and
 * returns the function that restores it. A proxy rather than a subclass, so `Error('...')` without
 * `new` keeps working; statics, `instanceof` and subclassing reach the original unchanged.
 */
function observeRegistrationErrors(observe) {
  const Original = globalThis.Error
  const seen = (error, args) => {
    if (args[0] === 'STACK_TRACE_ERROR') observe(error.stack)
    return error
  }
  globalThis.Error = new Proxy(Original, {
    construct: (target, args, newTarget) =>
      seen(Reflect.construct(target, args, newTarget === globalThis.Error ? target : newTarget), args),
    apply: (target, self, args) => seen(Reflect.apply(target, self, args), args),
  })
  return () => {
    globalThis.Error = Original
  }
}

/**
 * Every test under `suite` with its `' > '`-joined title path, every test and suite with its line,
 * and the suites' hook counts.
 */
function walk(suite, path, tests, tasks, suiteHooks) {
  const hooks = TestRunner.getSuiteHooks(suite) ?? {}
  for (const kind of KINDS) suiteHooks[kind] += hooks[kind]?.length ?? 0
  for (const task of suite.tasks ?? []) {
    const title = [...path, task.name]
    tasks.push({ kind: task.type === 'suite' ? 'suite' : 'test', line: task.location?.line ?? null })
    if (task.type === 'suite') walk(task, title, tests, tasks, suiteHooks)
    else tests.push({ title: title.join(' > '), mode: task.mode })
  }
}

/** Errors Vitest recorded while collecting `task` or any task under it. */
function collectErrors(task) {
  const own = (task.result?.errors ?? []).map((error) => String(error?.message ?? error))
  return [...own, ...(task.tasks ?? []).flatMap(collectErrors)]
}

export default class CollectionProbeRunner extends TestRunner {
  // Keyed by the file task, not its path: two projects may collect the same file.
  registrations = new Map()
  current = null
  restore = null
  workerStubReads = new Set()

  // Describe callbacks run after `importFile` returns, so registrations are observed from the
  // start of a file's collection to the end of the whole collection, attributed to the file whose
  // collection started last: a worker collects its files one after another.
  onCollectStart(file) {
    // Without isolation a module loads once per worker and every later file shares it, so a stub
    // read once (whole, or a value a helper keeps) may reach a later file without a read of its own:
    // every later file counts as having made every read made before it in the worker.
    const shared = this.config.isolate === false ? [...this.workerStubReads] : []
    this.current = { file: file.filepath, sink: [], stubReads: shared }
    this.registrations.set(file, this.current)
    globalThis.__vitestDedupStubRead = (specifier, key) => {
      this.current.stubReads.push(`${specifier}#${key}`)
      this.workerStubReads.add(`${specifier}#${key}`)
    }
    this.restore ??= observeRegistrationErrors((stack) => {
      const found = registration(stack)
      if (found) this.current.sink.push(found)
    })
    return super.onCollectStart(file)
  }

  async onCollected(files) {
    this.restore?.()
    this.restore = null
    delete globalThis.__vitestDedupStubRead
    if (!OUT) throw new Error('VITEST_DEDUP_ORACLE_OUT is not set')
    for (const file of files) {
      const tests = []
      const tasks = []
      const suiteHooks = Object.fromEntries(KINDS.map((kind) => [kind, 0]))
      walk(file, [], tests, tasks, suiteHooks)
      appendFileSync(
        OUT,
        JSON.stringify({
          file: file.filepath,
          project: file.projectName ?? '',
          tests,
          tasks,
          registrations: this.registrations.get(file)?.sink ?? [],
          stubReads: [...new Set(this.registrations.get(file)?.stubReads ?? [])],
          suiteHooks,
          collectErrors: collectErrors(file),
        }) + '\n',
      )
    }
    return super.onCollected?.(files)
  }
}
