// The next engine's duplicate rules, evaluated pairwise against the owned corpus: a gate over
// `fixtures/duplicates`, `fixtures/observed` and `fixtures/mixed`.
//
// The shared fixture headers pin the default engine -- nine rules, one verdict vocabulary -- so
// they cannot say what the next engine, whose ported rules are listed below, should do. This gate reads the
// same headers and judges only the ported rules' pairwise claims, the way the spike evaluator
// did: a positive expectation for any ported rule requires its
// participants connected by that rule, in the spanning pairs it reports; a `rule=none` expectation forbids any ported rule from
// linking its pairs; anything else the next engine links is a delta someone must adjudicate --
// a fixture to fix, or an entry in `next-duplicates-known.json` with a reason. A known entry
// whose delta stops being observed fails the gate, so an entry dies the moment the header or
// the behavior moves.
//
// A rule outside the ported set emitting under the next engine is a failure outright: its
// expectations must move through the corpus gate's engine axis, never be satisfied silently.
//
// Nothing here judges a run pass or fail except the floors the caller asserts on top: a
// contract violation is data, a measurement that did not measure is an error.

import { describeMatcher, findingMatches } from './recall-runner.mjs'

/** The rules the next engine runs. */
export const PORTED_RULES = new Set(['duplicate-test', 'duplicate-test-name', 'duplicate-each-row', 'duplicate-suite', 'parameterization-candidate', 'near-duplicate-test', 'subsumed-test', 'redundant-hook', 'cross-file-duplicate'])

/** The identity one participant is matched by, the way the spike evaluator matched: file:line. */
function endpoint(participant) {
  return `${participant.file}:${participant.line}`
}

/** The stable identity of an undirected pair of participants: both endpoints, sorted. */
export function edgeKey(left, right) {
  return [left, right].sort().join('|')
}

/**
 * The pairs of participants the next engine's ported rules linked, from one run's JSONL records.
 *
 * Every pair of locations a finding carries becomes an edge, so a cluster finding covers all of
 * its member pairs at once -- the same expansion the spike evaluator did.
 *
 * @param {object[]} records The parsed jsonl records of one run.
 * @returns {Map<string, Set<string>>} edge -> the ported rules that linked it.
 */
export function pairEdges(records) {
  const linked = new Map()
  for (const record of records) {
    if (record?.type !== 'finding' || !PORTED_RULES.has(record.rule)) continue
    const points = (record.locations ?? []).map((location) => endpoint({ file: basenameOf(location.path), line: location.line }))
    for (const [left, right] of combinations(points)) {
      const key = edgeKey(left, right)
      if (!linked.has(key)) linked.set(key, new Set())
      linked.get(key).add(record.rule)
    }
  }
  return linked
}

/** Basename of a report path, forward-slash or platform-native, the way the corpus gate matches. */
function basenameOf(path) {
  return String(path).split(/[\\/]/).pop()
}

/** Every 2-element combination of `items`, in order. */
function* combinations(items) {
  for (let i = 0; i < items.length; i += 1) {
    for (let j = i + 1; j < items.length; j += 1) {
      yield [items[i], items[j]]
    }
  }
}

/**
 * The findings the next engine reported under a rule it does not port. One of these is a
 * pending-by-rule expectation being satisfied silently, which is exactly what must fail.
 *
 * @param {object[]} records The parsed jsonl records of one run.
 * @returns {object[]} the finding records whose rule is not ported.
 */
export function strayFindings(records) {
  return records.filter((record) => record?.type === 'finding' && !PORTED_RULES.has(record.rule))
}

/**
 * What the parsed fixtures claim about the ported rules, as pair edges.
 *
 * A positive ported-rule expectation requires each of its participant pairs linked by that
 * rule. A `rule=none` expectation (distinct or not-compared) forbids every ported rule from
 * linking its pairs, except the pairs another expectation already requires -- a pair may carry
 * two rules' expectations, and the required half wins. Pending expectations and expectations of
 * rules the next engine does not port are skipped: the first are the corpus gate's ratchet, the
 * second are silent under next by design.
 *
 * @param {Map<string, object>} fixtures Parsed fixtures by file name (`parseFixture` results).
 * @returns {{required: {edge: string, rule: string, from: string}[], declared: {edge: string, rule: string}[], forbidden: {edge: string, from: string}[]}}
 */
export function expectationEdges(fixtures) {
  const required = []
  const forbidden = []
  const declared = []
  for (const [name, fixture] of fixtures) {
    for (const expectation of fixture.expectations ?? []) {
      if (expectation.pending) continue
      // In the engine's group order: files by bytes, lines by number. The serialized form sorts
      // line 9 last, and a locale puts `a.test.ts` before `B.test.ts`.
      const points = [...(expectation.participants ?? [])]
        .sort((a, b) => Buffer.compare(Buffer.from(a.file), Buffer.from(b.file)) || a.line - b.line)
        .map(endpoint)
      if (points.length < 2) continue
      if (PORTED_RULES.has(expectation.rule)) {
        // A group of up to four is reported as a chain of neighbouring pairs, so only neighbours
        // are required; every other pair is declared, and a cluster finding may link it.
        for (const [left, right] of combinations(points)) declared.push({ edge: edgeKey(left, right), rule: expectation.rule })
        for (let i = 1; i < points.length; i += 1) {
          required.push({ edge: edgeKey(points[i - 1], points[i]), rule: expectation.rule, from: `${name}` })
        }
      } else if (expectation.rule === 'none') {
        for (const [left, right] of combinations(points)) {
          forbidden.push({ edge: edgeKey(left, right), from: `${name}` })
        }
      }
    }
  }
  const requiredEdges = new Set(required.map((r) => r.edge))
  return { required, declared, forbidden: forbidden.filter((f) => !requiredEdges.has(f.edge)) }
}

/** Rules whose reported titles must match the runtime rendering, even at identical locations. */
const renderedRule = (rule) => rule === 'duplicate-test-name' || rule === 'duplicate-each-row'

/** The sorted `path:line` of a finding's locations. */
const locationsOf = (finding) => finding.locations.map((location) => `${location.path}:${location.line}`).sort()

/**
 * Body findings retain their source pattern; name and row findings carry exact rendered titles, which
 * must be checked even when two different title groups share the same declaration locations.
 */
const readable = (expected) => (expected.locations && !renderedRule(expected.rule) ? { ...expected, titles: undefined } : expected)

/** A rendered title distinguishes two name or row groups at the same pair of declarations. */
const findingIdentity = (finding) => locationsOf(finding).join(',') + (renderedRule(finding.rule) ? `|${JSON.stringify(finding.titles)}` : '')

/** Missing name and row groups also stay distinct when their expected declaration locations agree. */
const matcherIdentity = (expected) => (expected.locations ?? expected.titles ?? []).join(',') + (renderedRule(expected.rule) && expected.locations && expected.titles ? `|${JSON.stringify(expected.titles)}` : '')

/**
 * Judges one recall case: its ported expectations against what the next engine reported.
 *
 * @param {object} testCase A manifest case.
 * @param {object[]} records The run's jsonl records.
 * @returns {{failures: {id: string, message: string}[], judged: number}} Each delta under the
 *   stable id a known entry names, and how many expectations were judged.
 */
export function judgeRecall(testCase, records) {
  const findings = records.filter((record) => record?.type === 'finding' && PORTED_RULES.has(record.rule))
  const ported = (list) => (list ?? []).filter((expected) => PORTED_RULES.has(expected.rule))
  const failures = []
  const delta = (kind, rule, where, text) => {
    failures.push({ id: `recall-${kind}|${testCase.name}|${rule}|${where}`, message: `FAIL recall ${testCase.name}: ${text}` })
  }
  const matched = new Set()
  const take = (expected) => {
    const found = findings.find((finding) => !matched.has(finding) && findingMatches(finding, readable(expected)))
    if (found) matched.add(found)
    return found
  }
  // A case that read fewer tests can pass its silences without having compared anything.
  const analyzed = records.find((record) => record?.type === 'summary')?.testsAnalyzed
  if (analyzed !== testCase.expectedTests) {
    delta('tests', 'all', String(analyzed), `the next engine analyzed ${analyzed} test(s), and the case expects ${testCase.expectedTests}`)
  }
  const expected = ported(testCase.expectedFindings)
  for (const matcher of expected) {
    if (!take(matcher)) {
      delta('miss', matcher.rule, matcherIdentity(matcher), `expected ${describeMatcher(matcher)}, which the next engine did not report`)
    }
  }
  for (const matcher of ported(testCase.knownMisses)) take(matcher)
  for (const matcher of ported(testCase.expectedAbsent)) {
    const found = findings.find((finding) => findingMatches(finding, readable(matcher)))
    if (found) {
      matched.add(found)
      delta('absent', matcher.rule, findingIdentity(found), `${describeMatcher(matcher)} must stay absent, and the next engine reported it`)
    }
  }
  for (const finding of findings.filter((candidate) => !matched.has(candidate))) {
    delta('extra', finding.rule, findingIdentity(finding), `the next engine reported ${finding.rule} at ${locationsOf(finding).join(', ')}, which the case does not expect`)
  }
  return { failures, judged: expected.length }
}

/**
 * How many expectations each rule the next engine does not port still holds.
 *
 * @param {{rule: string}[]} expectations Expectations of the fixture headers and recall cases.
 * @returns {Record<string, number>} Count per rule.
 */
export function pendingByRule(expectations) {
  const counts = {}
  for (const { rule } of expectations) {
    if (rule !== 'none' && !PORTED_RULES.has(rule)) counts[rule] = (counts[rule] ?? 0) + 1
  }
  return counts
}

/**
 * Why the entries of `next-duplicates-known.json` cannot stand: each names a delta and says why.
 *
 * @param {unknown} known The parsed `cases` list.
 * @returns {string[]} One line per problem.
 */
export function knownProblems(known) {
  if (!Array.isArray(known)) return ['the known list must be a list of cases']
  return known.flatMap((entry, index) =>
    typeof entry?.id === 'string' && entry.id !== '' && typeof entry?.reason === 'string' && entry.reason.trim() !== ''
      ? []
      : [`known entry ${index} needs an id and a reason`],
  )
}

/**
 * Judges one run against the corpus's claims about the ported rules.
 *
 * Every failure carries the stable `id` a `next-duplicates-known.json` entry names, so a listed
 * delta is held (and printed) while it persists, fails unlisted, and a listed entry whose delta
 * is gone makes the gate fail until the entry is removed.
 *
 * @param {object} input
 * @param {{edge: string, rule: string, from: string}[]} input.required Edges that must be linked.
 * @param {{edge: string, rule: string}[]} [input.declared] Edges a positive expectation names
 *   without requiring them, with its rule.
 * @param {{edge: string, from: string}[]} input.forbidden Edges no ported rule may link.
 * @param {Map<string, Set<string>>} input.linked What the run linked.
 * @param {{id: string, reason: string}[]} input.known The adjudicated deltas.
 * @param {string} input.scope The directory the run covered, part of every id.
 * @returns {{failures: {id: string, message: string}[], held: string[], consumed: Set<string>}}
 */
export function judge({ required, declared = [], forbidden, linked, known, scope }) {
  const failures = []
  const fired = new Set()
  const missed = new Set()
  const miss = (edge, rule, from) => {
    const id = `miss|${scope}|${edge}|${rule}`
    fired.add(id)
    missed.add(edge)
    failures.push({
      id,
      message: `FAIL ${from}: expected ${rule} to link ${edge.replaceAll('|', ' <-> ')}; the next engine linked ${
        [...(linked.get(edge) ?? [])].join(', ') || 'nothing'
      }`,
    })
  }
  for (const { edge, rule, from } of required) {
    if (!linked.get(edge)?.has(rule)) miss(edge, rule, from)
  }
  for (const { edge, from } of forbidden) {
    const rules = linked.get(edge)
    if (rules !== undefined) {
      const id = `link|${scope}|${edge}`
      fired.add(id)
      failures.push({
        id,
        message: `FAIL ${from}: expected silence between ${edge.replaceAll('|', ' <-> ')}; the next engine linked them (${[...rules].join(', ')})`,
      })
    }
  }
  const claims = [...required, ...declared]
  const claimed = new Set(claims.map(({ edge, rule }) => `${edge}|${rule}`))
  const named = new Set(claims.map(({ edge }) => edge))
  const silenced = new Set(forbidden.map(({ edge }) => edge))
  for (const [edge, rules] of linked) {
    if (silenced.has(edge)) continue
    // A pair an expectation names is still judged per rule: the other rule linking it is a claim
    // nothing made. A miss already names what linked the pair instead.
    for (const rule of rules) {
      if (named.has(edge) && !missed.has(edge) && !claimed.has(`${edge}|${rule}`)) {
        const id = `rule|${scope}|${edge}|${rule}`
        fired.add(id)
        failures.push({
          id,
          message: `FAIL ${scope}: ${rule} linked ${edge.replaceAll('|', ' <-> ')}, which the fixture expects from another rule`,
        })
      }
    }
    if (named.has(edge)) continue
    const id = `extra|${scope}|${edge}`
    fired.add(id)
    failures.push({
      id,
      message: `FAIL ${scope}: the next engine linked ${edge.replaceAll('|', ' <-> ')} (${[...rules].join(', ')}), which no fixture expectation declares -- adjudicate it into a fixture header or a known entry`,
    })
  }
  const consumed = new Set(known.filter((entry) => fired.has(entry.id)).map((entry) => entry.id))
  const held = known
    .filter((entry) => consumed.has(entry.id))
    .map((entry) => `next-duplicates known: ${entry.id} -- ${entry.reason}`)
  return {
    failures: failures.filter((failure) => !consumed.has(failure.id)),
    held,
    consumed,
  }
}
