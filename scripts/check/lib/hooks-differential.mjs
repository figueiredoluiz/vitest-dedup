// Generated helpers for the hooks-only differential check (`check-hooks-differential.mjs`).
//
// The next engine reads a call of a helper as `hook-wrapper` (registers only hooks, so no test)
// when the helper's body runs nothing but hook registrations. That is a claim about every program
// the body can run, and each way around it found so far hid a test Vitest collects. This module
// writes one case per way around it ("route") and per way of declaring and reaching the helper
// ("context"), plus closed bodies the claim admits, and judges what Vitest and the analyzer report
// for them. Routes are the classes found so far; a new finding adds a route.
//
// Every case is a directory: `support.ts` (whose `cases()` registers the hidden test),
// `helper.ts` (and `helper-impl.ts` behind a barrel), and `case.test.ts`, which calls the helper
// on `CALL_LINE` inside a describe next to one visible test.

/** The line of `case.test.ts` that calls the helper. */
export const CALL_LINE = 4

const SUPPORT = `import { test } from 'vitest'
export function cases(..._args: unknown[]) { test('hidden', () => {}) }
export const coerced = { valueOf() { cases(); return 1000 } }
`

/**
 * Ways a call of the helper can run `cases()`. `prelude` is module code before the helper, `params`
 * its parameter list, `body` its statements after a `beforeEach`. `define` replaces the whole helper
 * module (the route is then tried in the `fn` context only); `call` replaces the test file's call;
 * `async` marks a route that needs an async helper.
 */
const ROUTES = [
  { id: 'call', body: 'cases()' },
  { id: 'namespace-member', prelude: "import * as support from './support'", body: 'support.cases()' },
  { id: 'method', prelude: 'const local = { run() { cases() } }', body: 'local.run()' },
  { id: 'getter-statement', prelude: 'const local = { get read() { cases(); return 0 } }', body: 'local.read' },
  { id: 'getter-initializer', prelude: 'const local = { get read() { cases(); return 0 } }', body: 'const value = local.read' },
  { id: 'getter-argument', prelude: 'const local = { get read() { cases(); return 0 } }', body: 'beforeEach(() => {}, local.read)' },
  { id: 'default-parameter', params: 'value = cases()', body: '' },
  { id: 'pattern-parameter', prelude: 'const local = { get read() { cases(); return 0 } }', params: '{ read } = local', body: '' },
  { id: 'conditional-callee', body: '(cases() ? beforeEach : beforeEach)(() => {})' },
  { id: 'hook-argument-call', body: 'beforeEach(() => {}, cases())' },
  { id: 'timeout-value-of', prelude: "import { coerced } from './support'", body: 'beforeEach(() => {}, coerced as unknown as number)' },
  { id: 'operator', prelude: "import { coerced } from './support'", body: 'const n = (coerced as unknown as number) + 1' },
  { id: 'if-test', prelude: "import { coerced } from './support'", body: 'if ((coerced as unknown as number) > 0) beforeEach(() => {})' },
  { id: 'template', prelude: 'const local = { get read() { cases(); return 0 } }', body: 'const text = `${local.read}`' },
  { id: 'spread', prelude: 'const list = { *[Symbol.iterator]() { cases() } }', body: 'const items = [...list]' },
  { id: 'for-of', prelude: 'const list = { *[Symbol.iterator]() { cases() } }', body: 'for (const item of list) {}' },
  {
    id: 'global-getter',
    prelude: "Object.defineProperty(globalThis, 'hiddenGlobal', { get() { cases(); return 0 }, configurable: true })\ndeclare const hiddenGlobal: number",
    body: 'const value = hiddenGlobal',
  },
  { id: 'async-then', async: true, prelude: 'const thenable = { get then() { cases(); return undefined } }', body: 'return thenable' },
  { id: 'reassigned-hook', prelude: 'let hook: Function = beforeEach\nhook = cases', body: 'hook(() => {})' },
  { id: 'alias-of-reassigned-hook', prelude: 'let hook: Function = beforeEach\nhook = cases\nconst alias = hook', body: 'alias(() => {})' },
  { id: 'reassigned-namespace', prelude: "import * as v from 'vitest'\nlet api: any = v\napi = { beforeEach: cases }", body: 'api.beforeEach(() => {})' },
  {
    id: 'reassigned-function',
    define: "import { beforeEach } from 'vitest'\nimport { cases } from './support'\nexport function setup() { beforeEach(() => {}) }\n// @ts-ignore\nsetup = cases\n",
  },
  {
    id: 'var-redeclaration',
    define: "import { beforeEach } from 'vitest'\nimport { cases } from './support'\nvar setup = function () { beforeEach(() => {}) }\n// @ts-ignore\nvar setup = cases\nexport { setup }\n",
  },
  {
    id: 'destructured-redeclaration',
    define: "import { beforeEach } from 'vitest'\nimport { cases } from './support'\nvar setup = function () { beforeEach(() => {}) }\n// @ts-ignore\nvar { run: setup } = { run: cases }\nexport { setup }\n",
  },
  {
    id: 'exported-hook-alias',
    define: "import { beforeEach } from 'vitest'\nexport const setup = beforeEach\n",
    call: 'setup(() => {}, coerced as unknown as number)',
  },
  { id: 'call-site-argument', body: '', call: 'setup(support.cases())' },
  { id: 'call-site-template', body: '', call: 'setup`${support.cases()}`' },
  { id: 'called-parameter', params: 'run: () => void', body: 'run()', call: 'setup(() => support.cases())' },
]

/** Bodies the hooks-only claim admits, each one edit away from a route. */
const CLOSED = [
  { id: 'hooks', body: 'afterEach(() => {})' },
  { id: 'state', body: "let n = 0\nconst state = { n, list: [n, 'a'], at: `t` }\nafterEach(() => {}, 1000)\nreturn state", returns: true },
  { id: 'branch', params: 'flag?: boolean', body: 'if (flag) { afterEach(() => {}) } else afterEach(() => {})' },
  { id: 'namespace-hook', prelude: "import * as v from 'vitest'", body: 'v.afterEach(() => {})' },
  { id: 'nested-hooks', prelude: 'function more() { afterEach(() => {}) }', body: 'more()' },
  { id: 'undefined', body: 'const unset = undefined' },
]

/** How a helper is declared and reached. */
const CONTEXTS = ['fn', 'arrow', 'nested', 'barrel', 'default', 'async']

const IMPORTS = "import { beforeEach, afterEach } from 'vitest'\nimport { cases } from './support'\n"

/**
 * The helper module for `route` in `context`.
 *
 * @param {{prelude?: string, params?: string, body?: string}} route The route or closed body.
 * @param {string} context One of `CONTEXTS`.
 * @returns {Record<string, string>} Helper files by name.
 */
function helperFiles(route, context) {
  const prelude = route.prelude ? `${route.prelude}\n` : ''
  const params = route.params ?? ''
  // Semicolons: a statement starting with `(` would otherwise call the previous one's result.
  const body = `beforeEach(() => {});\n  ${(route.body ?? '').split('\n').join(';\n  ')}`
  const head = `${IMPORTS}${prelude}`
  const declare = `function setup(${params}) {\n  ${body}\n}\n`
  return {
    fn: { 'helper.ts': `${head}export ${declare}` },
    arrow: { 'helper.ts': `${head}export const setup = (${params}) => {\n  ${body}\n}\n` },
    nested: { 'helper.ts': `${head}function inner(${params}) {\n  ${body}\n}\nexport function setup() {\n  afterEach(() => {})\n  inner()\n}\n` },
    barrel: { 'helper-impl.ts': `${head}export ${declare}`, 'helper.ts': "export { setup } from './helper-impl'\n" },
    default: { 'helper.ts': `${head}export default function (${params}) {\n  ${body}\n}\n` },
    async: { 'helper.ts': `${head}export async ${declare}` },
  }[context]
}

/**
 * The test file calling the helper on `CALL_LINE`.
 *
 * @param {string} context How the helper is exported (`default` imports it as the default).
 * @param {string} call The call statement.
 * @returns {string} The source of `case.test.ts`.
 */
function testFile(context, call) {
  const helper = context === 'default' ? "import setup from './helper'" : "import { setup } from './helper'"
  return `import { describe, test } from 'vitest'\n${helper}; import * as support from './support'; import { coerced } from './support'\ndescribe('case', () => {\n  ${call}\n  test('visible', () => {})\n})\n`
}

/**
 * Every case the check runs, in a fixed order.
 *
 * @returns {{id: string, open: boolean, files: Record<string, string>}[]} Cases with their files,
 *   by path relative to the case directory. An open case's call runs `cases()`; a closed one's
 *   registers only hooks.
 */
export function generateCases() {
  const generated = []
  for (const route of ROUTES) {
    const contexts = route.define || route.call ? ['fn'] : route.async ? ['async'] : CONTEXTS
    for (const context of contexts) {
      const helper = route.define ? { 'helper.ts': route.define } : helperFiles(route, context)
      generated.push({
        id: `open-${route.id}-${context}`,
        open: true,
        files: { 'support.ts': SUPPORT, ...helper, 'case.test.ts': testFile(context, route.call ?? 'setup()') },
      })
    }
  }
  for (const closed of CLOSED) {
    // An async helper's returned value is resolved, which the claim refuses.
    for (const context of CONTEXTS.filter((c) => !(closed.returns && c === 'async'))) {
      generated.push({
        id: `closed-${closed.id}-${context}`,
        open: false,
        files: { 'support.ts': SUPPORT, ...helperFiles(closed, context), 'case.test.ts': testFile(context, 'setup()') },
      })
    }
  }
  return generated
}

/**
 * Judges the cases against what Vitest collected and the sites the analyzer reported.
 *
 * @param {{id: string, open: boolean}[]} cases The generated cases.
 * @param {Map<string, {tests: string[]}>} vitestFiles Vitest's view, keyed `<id>/case.test.ts`.
 * @param {{disagreements: {path: string, kind: string, item: string}[], compared: string[]}} located
 *   The oracle's `compareSites` result for the same files.
 * @param {Map<string, {kind: string, line: number}[]>} sites The analyzer's sites by file.
 * @param {{id: string, reason: string}[]} known Cases expected to fail, with their reasons.
 * @returns {{failures: string[], unsound: string[], open: number, closed: number}} Failure
 *   messages (none when the check passes), the open cases whose call is not a `wrapper` site, and
 *   the counts.
 */
export function judge(cases, vitestFiles, located, sites, known) {
  const failures = []
  const unsound = []
  const compared = new Set(located.compared)
  // An open case's call registers the hidden test by construction, so it is judged by the call's
  // own site: Vitest places some hidden tasks on no line, and the site comparison then accepts
  // any module load for them. Every other test and suite must still have its site.
  for (const d of located.disagreements.filter((d) => d.kind === 'missing-site' && d.item !== `test@${CALL_LINE}`)) {
    failures.push(`${d.path.split('/')[0]}: no site for ${d.item}, which no route hides`)
  }
  const callKinds = (path) => (sites.get(path) ?? []).filter((site) => site.line === CALL_LINE).map((site) => site.kind)
  const knownIds = new Set(known.map((entry) => entry.id))
  const uncompared = new Set()
  for (const { id, open } of cases) {
    const path = `${id}/case.test.ts`
    const tests = vitestFiles.get(path)?.tests ?? []
    if (!compared.has(path)) {
      failures.push(`${id}: not compared (Vitest could not collect it); generated cases must collect`)
      uncompared.add(id)
      continue
    }
    if (open) {
      if (!tests.includes('case > hidden')) failures.push(`${id}: Vitest collected no hidden test, so the route proves nothing`)
      if (!callKinds(path).includes('wrapper')) unsound.push(id)
    } else {
      if (tests.length !== 1) failures.push(`${id}: a closed case collected ${JSON.stringify(tests)}`)
      if (!callKinds(path).includes('hook-wrapper')) {
        failures.push(`${id}: the call is ${JSON.stringify(callKinds(path))}, not hook-wrapper; the check proves nothing for it`)
      }
    }
  }
  for (const id of unsound) {
    if (!knownIds.has(id)) failures.push(`${id}: Vitest collects a test the analyzer counts nowhere (hook-wrapper hid it)`)
  }
  for (const id of knownIds) {
    if (!unsound.includes(id) && !uncompared.has(id)) failures.push(`${id}: listed as failing and now passes; delete it from hooks-differential-known.json`)
  }
  return { failures, unsound, open: cases.filter((c) => c.open).length, closed: cases.filter((c) => !c.open).length }
}
