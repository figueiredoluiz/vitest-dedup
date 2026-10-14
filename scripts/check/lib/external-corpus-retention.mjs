// Pure helpers for the external corpus's hand-reviewed true-positive retention checks.
//
// The default scan owns stable count and identity baselines. A named positive that is absent
// there may belong to a rule that ships `Off`; the caller can use `supplementalRetentionRules`
// to enable only that rule in a separate scan without contaminating the default baseline.
import { ALL_RULES, OFF_BY_DEFAULT_RULES, VERDICT_RULE } from './fixture-directives.mjs'

/** Parse every `<path>:<line>` and subsequent same-path `:<line>` from one prose entry. */
export function parseLocationRefs(text) {
  const refs = []
  let lastPath = null
  for (const token of text.split(/\s+/)) {
    const withPath =
      token.match(/^([^\s():]+\.(?:ts|tsx|js|jsx|mjs|cjs)):([1-9]\d*)[.,;:]*$/) ??
      token.match(/^\(([^\s():]+\.(?:ts|tsx|js|jsx|mjs|cjs)):([1-9]\d*)\)[.,;:]*$/)
    if (withPath) {
      lastPath = withPath[1]
      refs.push({ path: lastPath, line: Number(withPath[2]) })
      continue
    }
    if (/^\(?[^\s():]+\.(?:ts|tsx|js|jsx|mjs|cjs):/i.test(token)) {
      throw new Error(`malformed location reference token "${token}"`)
    }
    const bareLine =
      token.match(/^:([1-9]\d*)[.,;:]*$/) ?? token.match(/^\(:([1-9]\d*)\)[.,;:]*$/)
    if (bareLine) {
      if (!lastPath) {
        throw new Error(`location shorthand has no preceding path in token "${token}"`)
      }
      refs.push({ path: lastPath, line: Number(bareLine[1]) })
    } else if (token.startsWith(':') || token.startsWith('(:')) {
      throw new Error(`malformed location reference token "${token}"`)
    }
  }
  return refs
}

/** The rule named by an entry's trailing `(rule, verdict)` clause, or `undefined`. */
export function clauseRule(entry) {
  const clause = /\(([a-z][a-z0-9-]*),\s+([a-z][a-z0-9-]*)\)[.,;:]?$/.exec(entry)
  if (clause) return clause[1]
  const trailing = /\(([^()]*)\)[.,;:]?$/.exec(entry)
  const trailingText = trailing?.[1].trim()
  const words = trailingText?.split(/\s+/) ?? []
  const commaParts = trailingText?.split(',').map((part) => part.trim()) ?? []
  const resemblesCommaClause =
    commaParts.length === 2 &&
    ((commaParts.every((part) => /^[a-z][a-z0-9-]*$/.test(part))) ||
      ALL_RULES.has(commaParts[0]) ||
      Object.hasOwn(VERDICT_RULE, commaParts[1]))
  const resemblesClause =
    resemblesCommaClause ||
    (words.length === 2 && Object.hasOwn(VERDICT_RULE, words[1]))
  if (resemblesClause) {
    throw new Error(`malformed trailing rule clause: "${entry}"`)
  }
  return undefined
}

/**
 * Replace backslashes with forward slashes and remove repeated leading ./ prefixes for
 * location comparisons.
 */
function normalizedPath(path) {
  return path.replaceAll('\\', '/').replace(/^(?:\.\/)+/, '')
}

/** Whether one finding of the named rule covers every declaration reference in the entry. */
export function isKnownTruePositiveCovered(refs, findings, rule) {
  return findings.some(
    (finding) =>
      (rule === undefined || finding.rule === rule) &&
      refs.every((ref) =>
        finding.locations.some(
          (location) => {
            const expected = normalizedPath(ref.path)
            const actual = normalizedPath(location.path)
            return (
              location.line === ref.line &&
              (actual === expected || actual.endsWith(`/${expected}`))
            )
          },
        ),
      ),
  )
}

const SUMMARY_COUNT_FIELDS = [
  'testsAnalyzed',
  'filesAnalyzed',
  'errors',
  'warnings',
  'unresolvedTests',
]

/**
 * Throw a report-schema error naming the field unless its value is an integer greater than or
 * equal to zero.
 */
function requireNonNegativeInteger(value, field) {
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(
      `summary field \`${field}\` is ${JSON.stringify(value)}, not a non-negative integer; ` +
        'the analyzer and this gate disagree about the report schema',
    )
  }
}

/** Validate the summary contract shared by default and supplemental corpus runs. */
export function validateAnalysisRecords(records) {
  const summaries = records.filter((record) => record.type === 'summary')
  if (summaries.length !== 1) {
    throw new Error(`the run emitted ${summaries.length} summary records; expected exactly one`)
  }
  const summary = summaries[0]
  if (typeof summary.incomplete !== 'boolean') {
    throw new Error('summary field `incomplete` is not a boolean')
  }
  for (const field of SUMMARY_COUNT_FIELDS) requireNonNegativeInteger(summary[field], field)
  requireNonNegativeInteger(
    summary.incompleteCauses?.filesGlobalsUndetermined,
    'incompleteCauses.filesGlobalsUndetermined',
  )
  return {
    summary,
    findings: records.filter((record) => record.type === 'finding'),
  }
}

/** Findings for one opt-in rule, after proving the supplemental run completed and measured work. */
export function supplementalFindingsForRule(records, rule) {
  const { summary, findings } = validateAnalysisRecords(records)
  if (summary.incomplete) {
    throw new Error('supplemental analysis summary is incomplete')
  }
  return findings.filter((finding) => finding.rule === rule)
}

/** Return entries not covered by this finding set. Malformed entries are rejected, never skipped. */
export function uncoveredKnownTruePositives(entries, findings, implementedRules) {
  const uncovered = []
  for (const entry of entries) {
    const refs = parseLocationRefs(entry)
    if (refs.length === 0) {
      throw new Error(`could not parse a <path>:<line> reference: "${entry}"`)
    }
    const rule = clauseRule(entry)
    if (rule !== undefined && !implementedRules.has(rule)) {
      throw new Error(`names a rule the analyzer does not implement ("${rule}"): "${entry}"`)
    }
    if (!isKnownTruePositiveCovered(refs, findings, rule)) uncovered.push(entry)
  }
  return uncovered
}

/** Distinct named rules whose missing entries merit one targeted Warning-severity retry. */
export function supplementalRetentionRules(entries) {
  return [...new Set(entries.map(clauseRule).filter((rule) => OFF_BY_DEFAULT_RULES.has(rule)))]
}
