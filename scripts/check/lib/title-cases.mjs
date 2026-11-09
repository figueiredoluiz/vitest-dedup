// Generated `.each`/`.for` title cases: a seeded generator, their source spelling, and the smaller
// cases a disagreement shrinks to.
//
// A case is `{ api, pattern, rows }`: `api` is `each` or `for`, `pattern` an array of pieces whose
// concatenation is the title template, and `rows` the table, each row `{ kind, cells }` where
// `kind` is `array`, `object` or `scalar` and `cells` holds `[key, source]` pairs (the key is
// `null` outside an object). Values are JavaScript source spellings, so a case can hold what JSON
// cannot: `-0`, `NaN`, `1n`, `undefined`, a lone surrogate.
import { mulberry32 } from './memory-corpus.mjs'

/** Literal title text, including characters that end or extend a `$` field name. */
export const TEXT = ['a', ' ', '-', 'x y', 'é', '→', 'に', '0', '01', '.', ',', '(', ')']
/** Every printf directive Vitest's formatter scans, and near misses. */
export const PRINTF = ['%s', '%d', '%i', '%f', '%j', '%o', '%O', '%c', '%%', '%#', '%$', '%', '%x']
/** `$` fields: named, positional, dotted, non-ASCII and malformed. */
export const FIELDS = ['$a', '$b', '$name', '$0', '$1', '$2', '$01', '$a.b', '$a.0', '$labelに', '$$', '$', '$a$b', '$_', '$1a']
/** Object row keys, spelled as source; quoted ones are keys a bare name cannot spell. */
export const KEYS = ['a', 'b', 'name', '0', '1', "'labelに'", "'a.b'", '_']

const zeros = (length) => `'${'0'.repeat(length)}'`
/** Values around Vitest's default 40-unit display cut, the only ones `taskTitleValueFormatTruncate` changes. */
export const LONG_VALUES = [zeros(39), zeros(40), zeros(41), `'${'😀'.repeat(20)}'`, `'${'😀'.repeat(21)}'`]
/** Cell values as source: every primitive kind, numeric edge spellings, lengths around the 40-unit display cut, and composites. */
export const VALUES = [
  '0', '1', '-1', '42', '1.5', '-0', '0.1', '1e21', '1e-7', 'NaN', 'Infinity', '-Infinity', '9007199254740993',
  "''", "'x'", "'two'", "'1.99999999999999999999'", "' 12 '", "'0x1f'", "'1e3'", "'-0'", "'é'", "'%s'", "'$a'",
  "'\\uD800'", "'\\uD83D\\uDE00'", ...LONG_VALUES,
  'true', 'false', 'null', 'undefined', '1n', '{ k: 1 }', '[1, 2]', '[]', '{}',
]
/** The directives, fields, keys and values drawn more often than the rest. */
export const COMMON_PRINTF = ['%s', '%d', '%i', '%f', '%j', '%o', '%#', '%$']
export const COMMON_FIELDS = ['$a', '$b', '$name', '$0', '$1']
export const COMMON_KEYS = ['a', 'b', 'name', '0']
export const COMMON_VALUES = ['0', '1', '-1', '1.5', "''", "'x'", "'two'", 'true', 'false', 'null', 'undefined']

/** The memory corpus's PRNG: the same seed draws the same cases on every machine. */
export const random = mulberry32

/**
 * One case drawn from `next`.
 * @param {() => number} next The PRNG.
 * @returns {{api: string, pattern: string[], rows: object[]}} The case.
 */
export function generate(next) {
  const pick = (list) => list[Math.floor(next() * list.length)]
  const count = (low, high) => low + Math.floor(next() * (high - low + 1))
  // One rare piece refuses the whole title, so a uniform draw leaves most rows unproved and the
  // common directives untested; common pieces and values are drawn more often.
  const weighted = (common, all, share) => (next() < share ? pick(common) : pick(all))
  const piece = () => {
    const draw = next()
    if (draw < 0.45) return pick(TEXT)
    if (draw < 0.8) return weighted(COMMON_PRINTF, PRINTF, 0.8)
    return weighted(COMMON_FIELDS, FIELDS, 0.6)
  }
  const value = () => weighted(COMMON_VALUES, VALUES, 0.5)
  const pattern = Array.from({ length: count(1, 5) }, piece)
  // One kind for the whole table usually; a mixed table sometimes, since Vitest spreads only uniform array rows.
  const tableKind = pick(['array', 'array', 'object', 'object', 'scalar', 'mixed'])
  const rows = Array.from({ length: count(1, 3) }, () => {
    const kind = tableKind === 'mixed' ? pick(['array', 'object', 'scalar']) : tableKind
    if (kind === 'scalar') return { kind, cells: [[null, value()]] }
    if (kind === 'array') return { kind, cells: Array.from({ length: count(0, 3) }, () => [null, value()]) }
    const keys = [...new Set(Array.from({ length: count(1, 3) }, () => weighted(COMMON_KEYS, KEYS, 0.6)))]
    return { kind, cells: keys.map((key) => [key, value()]) }
  })
  return { api: pick(['each', 'each', 'for']), pattern, rows }
}

/**
 * Whether `testCase` holds a value a non-default display cut can render differently.
 * @param {{rows: object[]}} testCase The case.
 * @returns {boolean} Whether it does.
 */
export function holdsLongValue({ rows }) {
  return rows.some(({ cells }) => cells.some(([, value]) => LONG_VALUES.includes(value)))
}

/**
 * The source of one row.
 * @param {{kind: string, cells: Array<[string|null, string]>}} row The row.
 * @returns {string} Its spelling.
 */
export function spellRow({ kind, cells }) {
  if (kind === 'scalar') return cells[0][1]
  if (kind === 'array') return `[${cells.map(([, value]) => value).join(', ')}]`
  return `{ ${cells.map(([key, value]) => `${key}: ${value}`).join(', ')} }`
}

/**
 * A test file registering `testCase`, its tests on line 2.
 * @param {{api: string, pattern: string[], rows: object[]}} testCase The case.
 * @returns {string} The file's source.
 */
export function spellCase({ api, pattern, rows }) {
  const table = `[${rows.map(spellRow).join(', ')}]`
  return `import { test } from 'vitest'\ntest.${api}(${table})(${JSON.stringify(pattern.join(''))}, () => {})\n`
}

/**
 * How the analyzer's titles for one case compare with what Vitest collected. Only a proved title
 * (`comparisonKeys.name` not null) is compared; an unproved one is a refusal, which is allowed.
 * @param {{tests: string[], collectErrors: unknown[]} | undefined} collected Vitest's view of the file.
 * @param {object[]} records The analyzer's `test` records for the file.
 * @returns {{status: string, proved: number, rows: number, mismatches: object[]}} `vitest-error`
 *   when Vitest collected nothing it could title and the analyzer proved nothing either, `phantom`
 *   when it proved a title for such a case, `count` when the row counts differ, `title` when a
 *   proved title differs, and `agree` otherwise.
 */
export function verdict(collected, records) {
  const ordered = records.toSorted((a, b) => (a.row ?? 0) - (b.row ?? 0))
  const provedRecords = ordered.filter((record) => record.comparisonKeys?.name !== null)
  const proved = provedRecords.length
  if (collected === undefined || collected.collectErrors.length > 0 || collected.tests.length === 0) {
    const mismatches = provedRecords.map((record) => ({ row: record.row, vitest: null, analyzer: record.title }))
    return { status: proved > 0 ? 'phantom' : 'vitest-error', proved, rows: 0, mismatches }
  }
  if (ordered.length !== collected.tests.length) {
    return { status: 'count', proved, rows: collected.tests.length, mismatches: [{ vitest: collected.tests, analyzer: ordered.map((r) => r.title) }] }
  }
  const mismatches = ordered.flatMap((record, row) =>
    record.comparisonKeys?.name !== null && record.title !== collected.tests[row]
      ? [{ row, vitest: collected.tests[row], analyzer: record.title }]
      : [])
  return { status: mismatches.length > 0 ? 'title' : 'agree', proved, rows: collected.tests.length, mismatches }
}

/** Replacement values, plainest first: a value shrinks only to one before it. */
const PLAINER = ['0', "'x'"]
const rank = (value) => (PLAINER.includes(value) ? PLAINER.indexOf(value) : PLAINER.length)

/**
 * The cases one step smaller than `testCase`: one pattern piece, row or cell dropped, or one value
 * replaced by a plainer one. Every candidate keeps at least one piece, row and object key.
 * @param {{api: string, pattern: string[], rows: object[]}} testCase The case.
 * @returns {Array<object>} The candidates, largest reduction first.
 */
export function shrinkCandidates(testCase) {
  const out = []
  const { pattern, rows } = testCase
  for (let i = 0; i < rows.length && rows.length > 1; i++) out.push({ ...testCase, rows: rows.toSpliced(i, 1) })
  for (let i = 0; i < pattern.length && pattern.length > 1; i++) out.push({ ...testCase, pattern: pattern.toSpliced(i, 1) })
  if (testCase.api === 'for') out.push({ ...testCase, api: 'each' })
  rows.forEach((row, r) => {
    const replaceRow = (cells) => ({ ...testCase, rows: rows.with(r, { ...row, cells }) })
    row.cells.forEach(([key, value], c) => {
      if (row.kind !== 'scalar' && (row.kind === 'array' || row.cells.length > 1)) out.push(replaceRow(row.cells.toSpliced(c, 1)))
      // Strictly plainer only, so shrinking cannot swap two plain values back and forth.
      for (const plainer of PLAINER.slice(0, rank(value))) out.push(replaceRow(row.cells.with(c, [key, plainer])))
    })
  })
  return out
}
