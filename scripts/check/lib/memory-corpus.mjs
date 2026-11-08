#!/usr/bin/env node
// A deterministic synthetic Vitest corpus, sized in tests, for the memory gates.
//
// `check-memory.mjs` governs **bytes retained per test**. That ratio is only a measurement if the
// corpus it is measured over is fixed, so this module generates one from a seed: same seed, same
// N, byte-identical tree on every machine and every commit. `corpusDigest` makes that an
// assertion rather than a claim -- the gates pin the digest, so a change to any weight or template
// below forces a deliberate edit of the recorded bytes-per-test next to it.
//
// ## Why not a real corpus
//
// `run-external-corpus.mjs` already pins real repositories, and they are the wrong input here for
// two reasons. They need network and ~1GB of clones, and a shallow clone of a moving tag is not
// byte-stable, so the denominator of a per-test ratio would drift under the gate. And `fixtures/`
// is far too small: 300-odd tests cannot separate a per-test constant from the fixed cost of
// starting the process.
//
// ## Why the mix is deliberately non-uniform
//
// A thousand copies of one test would measure **interning**, not the IR. Identical bodies collapse
// to one alpha fingerprint, identical specifiers to one resolved identity, and the measured
// bytes-per-test would fall as N grew -- a corpus that makes the scaling test pass by construction
// while the real per-test cost is unbounded. Everything here is therefore keyed on the test's
// global index, and every quantity whose cost is shared across tests -- helper modules, packages,
// resolved import identities -- scales with N rather than staying fixed.
//
// The weights below target a legacy suite, which is what this tool exists for. Each one is a
// claim about what such a suite looks like, and each one moves the measured number:
//
//   - **Suite nesting** (mean depth ~1.35, tail to 3). `TestCase::suite_path` is a `Vec<Title>`
//     per test, so nesting is retained per test, not per describe. A flat corpus would understate
//     it and a uniformly deep one would overstate it.
//   - **Hooks on 55% of files**, some on inner describes. `Context::hooks` carries a fingerprinted
//     body per hook and is retained on *every test the hook applies to*, so this is the single
//     biggest multiplier in the record's `fields` breakdown and the easiest one to regress.
//   - **`vi.mock` on 30% of files**, 1-3 specifiers, half with a factory. Populates
//     `Context::mocked_modules`, and the factory form is what makes the mock body non-trivial.
//   - **`.each` / `.for` rows on 12% of tests**, tables of 2-8 rows in three literal forms. Rows
//     share a declaration and a location but are separate `TestCase`s, which is the one shape
//     where per-test retention and per-declaration retention visibly diverge.
//   - **Body size with a long tail**: 45% of 2-4 statements, 40% of 5-12, 12% of 13-30, 3% of
//     31-60. A mean is not enough -- the fingerprint cost of the tail is what a `String` added to
//     the IR would show up in first, and a corpus of uniformly small bodies would hide it.
//   - **Four title forms**, because `Title` is an enum whose variants do not cost the same.
//   - **Modifiers on 8% of tests**, skewed to `.skip` and `.todo` the way a legacy suite is.
//   - **10% of bodies are verbatim repeats** of an earlier body in the same package. Real legacy
//     suites are full of them, they are what this tool looks for, and they are also the honest
//     amount of interning to leave in: with none, the corpus would be unrepresentatively hostile;
//     with much more, it would measure the intern table.
//
// Files hold 4-24 tests and live at `packages/p<n>/src/<area>/<name>.test.ts`, with helpers beside
// them at `packages/p<n>/src/helpers/h<k>.ts`. Packages scale with the file count (one per ~60
// files, six helper modules each), so the number of *distinct* resolved module identities grows
// with N. Pinning the package count instead would let import-identity cost amortize away as N
// grew, which is exactly the sublinearity this corpus must not manufacture.
import { createHash } from 'node:crypto'
import { mkdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'

/**
 * Total length, in characters, that `corpusRoot` pads every corpus path to.
 *
 * **This is not cosmetic, it is the difference between a measurement and a number.** A resolved
 * import identity is retained as `resolved:<canonical absolute path>`, so retained bytes depend on
 * where the repository happens to be checked out. Measured: moving the same 1,000-test corpus from
 * a 6-character root to a 59-character one moved `bytesPerTest` from 2836 to 2962 -- 4.4%, for
 * nothing but the checkout location. A CI runner (`/home/runner/work/vitest-dedup/vitest-dedup`)
 * and a developer's machine differ by more than that, so an un-normalized ratchet would either go
 * red on arrival in CI or need a tolerance wide enough to hide a real regression.
 *
 * Padding the corpus root to a constant absolute length removes the variable instead of absorbing
 * it, which is why the ratchet below can be near-zero-slack. 160 leaves room for a long checkout
 * path and keeps the deepest generated file (~45 further characters) well inside every platform
 * limit that matters.
 */
const PATH_BUDGET = 160

/**
 * The padded, fixed-length absolute directory a corpus of `tests` tests lives in.
 *
 * The leaf is fixed-width too (`n-0000010000`), so the gate and the scaling test measuring the
 * same N measure the same path -- and therefore the same number -- rather than differing by the
 * length of the word in front of it.
 *
 * @throws {Error} when `base` is already too long to pad. Failing is correct: silently measuring
 *   at a different path length would produce a plausible number that is not comparable to the
 *   recorded one, which is the whole failure mode this function exists to prevent.
 */
export function corpusRoot(base, tests) {
  mkdirSync(base, { recursive: true })
  const canonical = realpathSync(base)
  const leaf = `n-${String(tests).padStart(10, '0')}`
  const padding = PATH_BUDGET - canonical.length - leaf.length - 2
  if (padding < 1) {
    throw new Error(
      `the corpus base path ${canonical} is ${canonical.length} characters, leaving no room to ` +
        `pad to the ${PATH_BUDGET}-character budget. Retained bytes include canonical absolute ` +
        'paths, so measuring at an unpadded length would produce a number that is not comparable ' +
        'to the recorded one. Check the repository out somewhere shorter, or raise PATH_BUDGET ' +
        'and re-measure every pinned figure in the same commit.',
    )
  }
  return join(canonical, 'p'.repeat(padding), leaf)
}

export const DEFAULT_SEED = 0x5ee1_1e55

/**
 * The scales the gates run at, and everything each one is pinned to.
 *
 * `digest` is this generator's output hash; `analyzedFiles` and `tests` are what the ANALYZER must
 * report over that tree. They live together because they are one fact -- what this corpus is --
 * and because a generator change that moves the digest must move the recorded memory numbers in
 * the same commit. `tests` is asserted as an equality rather than a floor: the tree is
 * byte-identical, so the extracted count is a constant, and a collapse in it would otherwise
 * shrink the denominator and make bytes-per-test look better.
 *
 * Measured 2026-09-19 with `lib/memory-corpus.mjs` at `DEFAULT_SEED`.
 */
export const CORPUS_SCALES = {
  1_000: {
    digest: 'de14f90a29a525d8a1ac4c09d542eec4f41bb88041526458f2a578adc42f4eb2',
    analyzedFiles: 70,
    tests: 1_000,
  },
  10_000: {
    digest: '6c005eca2666b16516ccce542308ef6b15eac352b13b3bdf4cdd850746cdb821',
    analyzedFiles: 727,
    tests: 10_000,
  },
  100_000: {
    digest: '975a267d081e9f81048792a6b2b5d2193fffdfe91bf50c43d4a2b81c0219bf28',
    analyzedFiles: 7_153,
    tests: 100_000,
  },
}

/**
 * mulberry32. Integer ops and one division by 2^32, so the stream is identical on every V8 and
 * every platform -- which is the whole point. Do not replace it with anything that touches
 * `Math.random`, the clock, or a float accumulator.
 */
export function mulberry32(seed) {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Draws from `[[value, weight], ...]`. Weights need not sum to anything in particular. */
function weighted(random, table) {
  const total = table.reduce((sum, entry) => sum + entry[1], 0)
  let point = random() * total
  for (const [value, weight] of table) {
    point -= weight
    if (point < 0) return value
  }
  return table[table.length - 1][0]
}

/** Inclusive integer range. */
const between = (random, low, high) => low + Math.floor(random() * (high - low + 1))

const WORDS = [
  'account', 'adapter', 'basket', 'billing', 'cache', 'catalog', 'checkout', 'cursor',
  'dispatch', 'draft', 'envelope', 'fixture', 'gateway', 'handler', 'invoice', 'journal',
  'ledger', 'locale', 'manifest', 'nonce', 'offer', 'parser', 'payload', 'quota',
  'receipt', 'registry', 'render', 'schema', 'session', 'shipment', 'sidebar', 'snapshot',
  'stream', 'ticket', 'token', 'tracker', 'upload', 'vendor', 'wallet', 'widget',
]

const AREAS = ['api', 'domain', 'services', 'ui', 'workers']

const HELPERS_PER_PACKAGE = 6
const FILES_PER_PACKAGE = 60
const TESTS_PER_FILE = [4, 24]
const DUPLICATE_BODY_RATE = 0.1
const RING = 64

const SUITE_DEPTH = [[0, 15], [1, 45], [2, 30], [3, 10]]
const BODY_SIZE = [[[2, 4], 45], [[5, 12], 40], [[13, 30], 12], [[31, 60], 3]]
const TITLE_FORM = [['plain', 70], ['template', 15], ['concat', 8], ['printf', 7]]
const MODIFIER = [[null, 92], ['skip', 3], ['todo', 2], ['concurrent', 2.5], ['only', 0.5]]
const EACH_FORM = [['rows', 45], ['objects', 40], ['for', 15]]
const HOOK_KIND = [['beforeEach', 46], ['afterEach', 24], ['beforeAll', 20], ['afterAll', 10]]

/**
 * Body statements. Each is a distinct AST shape -- call, await, destructuring, branch, loop,
 * try/catch, spy, snapshot -- because the extractor's per-node cost is not uniform and a corpus
 * built from one statement shape would measure that one shape.
 *
 * Every template embeds `i` (the test's global index) so bodies are textually distinct unless the
 * generator deliberately repeats one.
 */
// Each template is self-contained: every binding it introduces carries the statement ordinal `s`,
// so a body never redeclares a `const`. That is not cosmetic -- a corpus of files with duplicate
// declarations is a corpus whose parse diagnostics, not whose IR, might be what is being measured.
//
// `id` is the test's global index as a ZERO-PADDED fixed-width string, used everywhere an
// identifier or a title needs to be unique; `v` is an N-independent number, used everywhere a
// numeric literal is needed (a literal cannot be zero-padded -- `020` is not valid strict-mode
// JavaScript). Between them, no generated token gets wider as N grows. That is load-bearing: the
// first measurement of this corpus grew 8.1% per decade purely because `case-9.test.ts` becomes
// `case-7866.test.ts` and those paths are retained, which would have been read as the analyzer
// scaling badly. Width-invariance is what makes the scaling test measure the analyzer instead of
// the generator.
const STATEMENTS = [
  ({ id, s, k, n }) => `  const value${id}_${s} = compute${k}(${n})\n  expect(value${id}_${s}).toBeLessThan(97)`,
  ({ v, k, n, word }) => `  expect(compute${k}(${n})).toBe(${v % 97}) // ${word}`,
  ({ id, s, k, word }) => `  const loaded${id}_${s} = await load${k}('${word}-${id}')\n  expect(loaded${id}_${s}.kind).toBe('${word}')`,
  ({ v, word }) => `  expect({ id: ${v}, kind: '${word}' }).toEqual({ id: ${v}, kind: '${word}' })`,
  ({ v, n, m }) => `  if (${n} > ${m}) {\n    expect(${v}).toBeGreaterThan(0)\n  }`,
  ({ k, v }) => `  for (const item of items${k}) {\n    expect(item.id).not.toBe(${v})\n  }`,
  ({ id, v, s }) => `  const spy${id}_${s} = vi.fn((input: number) => input + ${v % 13})\n  spy${id}_${s}(${v})\n  expect(spy${id}_${s}).toHaveBeenCalledTimes(1)`,
  ({ v, k }) => `  await expect(fetch${k}(${v})).resolves.toMatchObject({ ok: true })`,
  ({ v, s, k }) => `  try {\n    risky${k}(${v})\n  } catch (error${s}) {\n    expect(error${s}).toBeInstanceOf(Error)\n  }`,
  ({ v, k, word }) => `  expect(format${k}(${v}, '${word}')).toBe('${word}:${v}')`,
  ({ id, v, s, word }) => `  const ${word}${id}_${s} = [${v}, ${v + 1}, ${v + 2}].map((entry) => entry * 2)\n  expect(${word}${id}_${s}).toHaveLength(3)`,
  ({ v, k }) => `  expect(() => risky${k}(${v})).not.toThrow()`,
  ({ v, s, word }) => `  const ${word}Result${s} = { total: ${v}, label: \`${word}-\${${v}}\` }\n  expect(${word}Result${s}.total).toBe(${v})`,
  ({ id, s, k, n }) => `  const rows${id}_${s} = [${n}, ${n + 1}].map((entry) => compute${k}(entry))\n  expect(rows${id}_${s}.every((entry) => entry >= 0)).toBe(true)`,
  ({ id, v, s, k, word }) => `  const ${word}Await${s} = await fetch${k}(${v})\n  expect(${word}Await${s}.id).toBe(${v})`,
]

/** One helper module. Exports everything a body template can reach for under suffix `k`. */
const helperSource = (k) => `// Generated helper module ${k}. Do not edit; see lib/memory-corpus.mjs.
export const items${k} = [{ id: ${k * 7 + 1} }, { id: ${k * 7 + 2} }]

export function compute${k}(input: number): number {
  return (input * ${k + 3}) % 97
}

export async function load${k}(key: string): Promise<{ id: number; kind: string }> {
  return { id: key.length, kind: key.split('-')[0] }
}

export async function fetch${k}(id: number): Promise<{ ok: boolean; id: number }> {
  return { ok: true, id }
}

export function risky${k}(input: number): number {
  if (input < 0) throw new Error('negative')
  return input
}

export function format${k}(input: number, label: string): string {
  return \`\${label}:\${input}\`
}
`

/** Zero-padded to a constant width, so a title's byte cost does not depend on how large N is. */
const label = (value) => String(value).padStart(7, '0')

/** Renders the title expression for one test, in one of four forms `Title` models differently. */
function renderTitle(form, index, word) {
  const id = label(index)
  if (form === 'template') return `\`${word} \${'case'} ${id}\``
  if (form === 'concat') return `'${word} ' + '${id}'`
  if (form === 'printf') return `'${word} handles %s for case ${id}'`
  return `'${word} ${id}'`
}

/** Renders a `.each` / `.for` table plus the callback parameter list that destructures it. */
function renderEach(form, random, index, word, rowCount) {
  const base = index % 997
  if (form === 'objects') {
    const rows = Array.from({ length: rowCount }, (_, row) =>
      `    { input: '${word}-${row}', want: ${base + row} },`)
    return { table: `[\n${rows.join('\n')}\n  ]`, params: '({ input, want })', call: 'test.each' }
  }
  const rows = Array.from({ length: rowCount }, (_, row) =>
    `    [${base + row}, ${row}, ${base + row * 2}],`)
  const call = form === 'for' ? 'test.for' : 'test.each'
  return { table: `[\n${rows.join('\n')}\n  ]`, params: '([left, right, sum])', call }
}

/** Renders one hook, at the given indentation, with a body of its own. */
function renderHook(random, kind, indent, index, helpers) {
  const lines = Array.from({ length: between(random, 1, 5) }, (_, line) => {
    const helper = helpers[between(random, 0, helpers.length - 1)]
    const word = WORDS[between(random, 0, WORDS.length - 1)]
    // `line` is in the binding name for the same reason the body templates carry `s`: two draws
    // of the same word inside one hook would otherwise redeclare a `const`.
    return `${indent}  const ${word}${label(index)}_${line} = compute${helper}(${between(random, 1, 400)})`
  })
  return `${indent}${kind}(() => {\n${lines.join('\n')}\n${indent}})\n`
}

/**
 * Generates a corpus of exactly `tests` tests under `root`.
 *
 * @returns {{root: string, tests: number, files: number, bytes: number, digest: string}}
 *   `digest` is SHA-256 over every generated `path\0content\0` in generation order. Two runs that
 *   agree on it generated the same tree, byte for byte.
 */
export function generateCorpus({ root, tests, seed = DEFAULT_SEED, clean = true }) {
  if (!Number.isInteger(tests) || tests < 1) {
    throw new Error(`generateCorpus needs a positive integer test count, got ${JSON.stringify(tests)}`)
  }
  if (clean) rmSync(root, { recursive: true, force: true })

  const random = mulberry32(seed)
  const hash = createHash('sha256')
  let bytes = 0
  let files = 0

  const emit = (path, text) => {
    const destination = join(root, path)
    mkdirSync(dirname(destination), { recursive: true })
    writeFileSync(destination, text)
    hash.update(path.split('\\').join('/'))
    hash.update('\0')
    hash.update(text)
    hash.update('\0')
    bytes += Buffer.byteLength(text)
    files += 1
  }

  // File and package counts are derived from N so that distinct module identities scale with it.
  const meanTestsPerFile = (TESTS_PER_FILE[0] + TESTS_PER_FILE[1]) / 2
  const fileEstimate = Math.max(1, Math.round(tests / meanTestsPerFile))
  const packages = Math.max(2, Math.round(fileEstimate / FILES_PER_PACKAGE))

  for (let pkg = 0; pkg < packages; pkg += 1) {
    for (let k = 0; k < HELPERS_PER_PACKAGE; k += 1) {
      emit(`packages/p${label(pkg)}/src/helpers/h${k}.ts`, helperSource(k))
    }
  }

  let index = 0
  let fileNumber = 0
  let ring = []
  let pkg = 0
  let filesInPackage = 0
  const filesPerPackage = Math.max(1, Math.ceil(fileEstimate / packages))

  while (index < tests) {
    if (filesInPackage >= filesPerPackage && pkg < packages - 1) {
      pkg += 1
      filesInPackage = 0
      ring = [] // Repeats stay inside a package: across packages the resolved imports differ.
    }
    const remaining = tests - index
    const want = Math.min(remaining, between(random, TESTS_PER_FILE[0], TESTS_PER_FILE[1]))
    const area = AREAS[between(random, 0, AREAS.length - 1)]
    // The helper suffixes this file imports. A repeated body may reference one the file did not
    // start with, so this grows as bodies are emitted and the import block is written afterwards
    // -- a repeat whose helper was left unimported would be a free identifier rather than a
    // resolved one, which is a different (and cheaper) thing to retain.
    const helperSet = new Set(Array.from(
      { length: between(random, 1, 3) },
      () => between(random, 0, HELPERS_PER_PACKAGE - 1),
    ))
    const helpers = () => [...helperSet].sort((a, b) => a - b)

    const hookKinds = new Set()
    const hookPlan = []
    if (random() < 0.55) {
      for (let n = between(random, 1, 3); n > 0; n -= 1) {
        const kind = weighted(random, HOOK_KIND)
        hookKinds.add(kind)
        hookPlan.push(kind)
      }
    }

    const mocks = []
    if (random() < 0.3) {
      for (let n = between(random, 1, 3); n > 0; n -= 1) {
        const target = between(random, 0, HELPERS_PER_PACKAGE - 1)
        mocks.push(random() < 0.5
          ? `vi.mock('../helpers/h${target}', () => ({\n  compute${target}: () => ${between(random, 1, 90)},\n  risky${target}: () => 0,\n}))`
          : `vi.mock('../helpers/h${target}')`)
      }
    }

    const body = []
    const depth = weighted(random, SUITE_DEPTH)
    let indent = ''
    for (let level = 0; level < depth; level += 1) {
      const word = WORDS[between(random, 0, WORDS.length - 1)]
      body.push(`${indent}describe('${word} level ${level} of file ${label(fileNumber)}', () => {`)
      indent += '  '
      if (level > 0 && random() < 0.3) {
        body.push(renderHook(random, 'beforeEach', indent, fileNumber * 10 + level, helpers()))
      }
    }
    for (const kind of hookPlan) {
      body.push(renderHook(random, kind, indent, fileNumber, helpers()))
    }

    let emitted = 0
    while (emitted < want) {
      const word = WORDS[between(random, 0, WORDS.length - 1)]
      const modifier = weighted(random, MODIFIER)
      const rows = random() < 0.12 ? Math.min(want - emitted, between(random, 2, 8)) : 0
      const isEach = rows >= 2
      const titleForm = isEach ? 'printf' : weighted(random, TITLE_FORM)
      const title = renderTitle(titleForm, index, word)

      if (!isEach && modifier === 'todo') {
        body.push(`${indent}test.todo(${title})`)
        index += 1
        emitted += 1
        continue
      }

      let statements
      if (ring.length > 0 && random() < DUPLICATE_BODY_RATE) {
        const repeat = ring[between(random, 0, ring.length - 1)]
        statements = repeat.text
        for (const suffix of repeat.suffixes) helperSet.add(suffix)
      } else {
        const [low, high] = weighted(random, BODY_SIZE)
        const used = new Set()
        const available = helpers()
        const lines = Array.from({ length: between(random, low, high) }, (_, s) => {
          const template = STATEMENTS[between(random, 0, STATEMENTS.length - 1)]
          const k = available[between(random, 0, available.length - 1)]
          used.add(k)
          return template({
            id: label(index),
            v: index % 997,
            s,
            k,
            n: between(random, 1, 400),
            m: between(random, 1, 400),
            word: WORDS[between(random, 0, WORDS.length - 1)],
          })
        })
        statements = lines.join('\n')
        ring.push({ text: statements, suffixes: [...used] })
        if (ring.length > RING) ring.shift()
      }
      const indented = indent === ''
        ? statements
        : statements.split('\n').map((line) => (line === '' ? line : indent + line)).join('\n')

      if (isEach) {
        const each = renderEach(weighted(random, EACH_FORM), random, index, word, rows)
        body.push(`${indent}${each.call}(${each.table})(${title}, async ${each.params} => {\n${indented}\n${indent}})`)
        index += rows
        emitted += rows
      } else {
        const suffix = modifier === null ? '' : `.${modifier}`
        body.push(`${indent}test${suffix}(${title}, async () => {\n${indented}\n${indent}})`)
        index += 1
        emitted += 1
      }
    }

    for (let level = depth; level > 0; level -= 1) {
      indent = indent.slice(2)
      body.push(`${indent}})`)
    }

    const parts = [`// Generated test file ${label(fileNumber)}. Do not edit; see lib/memory-corpus.mjs.`]
    parts.push(`import { ${['describe', 'test', 'expect', 'vi', ...hookKinds].join(', ')} } from 'vitest'`)
    for (const helper of helpers()) {
      const names = [`compute${helper}`, `load${helper}`, `fetch${helper}`, `risky${helper}`,
        `format${helper}`, `items${helper}`]
      parts.push(`import { ${names.join(', ')} } from '../helpers/h${helper}'`)
    }
    parts.push('')
    if (mocks.length > 0) parts.push(...mocks, '')

    emit(`packages/p${label(pkg)}/src/${area}/case-${label(fileNumber)}.test.ts`, `${[...parts, ...body].join('\n')}\n`)
    fileNumber += 1
    filesInPackage += 1
  }

  return { root, tests, files, bytes, digest: hash.digest('hex') }
}

// Standalone generation, for inspecting a corpus by hand:
//   node scripts/check/lib/memory-corpus.mjs <root> <tests> [seed]
if (process.argv[1] && process.argv[1].endsWith('memory-corpus.mjs')) {
  const [root, count, seed] = process.argv.slice(2)
  if (!root || !count) {
    console.error('usage: memory-corpus.mjs <root> <tests> [seed]')
    process.exit(2)
  }
  const result = generateCorpus({
    root,
    tests: Number(count),
    seed: seed === undefined ? DEFAULT_SEED : Number(seed),
  })
  console.log(`${result.tests} tests in ${result.files} files, ${result.bytes} bytes, sha256 ${result.digest}`)
}
