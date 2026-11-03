// Versioned owned-core evidence shared by corpus measurement and isolation checks.
const TEST_KEYS = ['duplicate', 'crossFile', 'parameterization', 'similarity', 'subsumption', 'name', 'rowClass', 'eventSet']
const VIEW_FIELDS = { alpha: 'body', structural: 'shape', behavior: 'trace', contextKey: 'context', modifierKey: 'modifiers' }
const digest = (value) => value === null || (typeof value === 'string' && /^[0-9a-f]{16}$/.test(value))

/** Whether a diagnostic record explicitly opts into a nonlegacy engine/version contract. */
export function isOwnedRecord(record) {
  return ['test', 'hook'].includes(record.type) && (record.engine !== undefined || (record.schemaVersion !== undefined && record.schemaVersion !== '0.1'))
}

/** Reject absent/unsupported evidence before a consumer can treat missing fields as equal. */
export function validateOwnedRecord(record) {
  if (!isOwnedRecord(record)) return
  if (record.engine !== 'owned' || record.schemaVersion !== '0.2') throw new Error('unsupported owned diagnostic record version/engine')
  if (typeof record.path !== 'string' || !Number.isInteger(record.line) || record.line < 1 || !Number.isInteger(record.column) || record.column < 1) throw new Error('owned record needs a source location')
  const test = record.type === 'test'
  if (test && (typeof record.title !== 'string' || typeof record.comparable !== 'boolean' || typeof record.traceComplete !== 'boolean' || !Number.isSafeInteger(record.ordinaryEvents) || record.ordinaryEvents < 0 || !(record.exclusion === null || typeof record.exclusion === 'string') || !(record.row === null || (Number.isSafeInteger(record.row) && record.row >= 0)))) throw new Error('owned test lacks its eligibility facts')
  if (!test && typeof record.kind !== 'string') throw new Error('owned hook needs its lifecycle kind')
  for (const field of test ? TEST_KEYS : ['effects']) {
    if (!digest(record.comparisonKeys?.[field])) throw new Error(`owned record lacks comparisonKeys.${field}`)
  }
  if (record.views !== null) {
    for (const field of test ? Object.values(VIEW_FIELDS) : ['body', 'shape', 'trace']) {
      if (!digest(record.views?.[field])) throw new Error(`owned record lacks views.${field}`)
    }
  }
  if (test && record.comparable !== (record.exclusion === null)) throw new Error('owned comparability disagrees with exclusion')
  if (test && record.comparable && ['duplicate', 'crossFile'].some((field) => record.comparisonKeys[field] === null)) throw new Error('comparable owned test lacks its exact comparison keys')
  if (test && !record.comparable && ['duplicate', 'crossFile', 'parameterization', 'similarity', 'subsumption'].some((field) => record.comparisonKeys[field] !== null)) throw new Error('excluded owned test carries comparison proof')
}

/** Use owned views only for dimensional relations, never as the legacy rule-eligibility oracle. */
export function isolationRecord(record) {
  validateOwnedRecord(record)
  if (!isOwnedRecord(record)) return record
  const projected = { ...record, trivial: ['empty-body', 'no-callback'].includes(record.exclusion) }
  for (const [field, view] of Object.entries(VIEW_FIELDS)) projected[field] = record.views?.[view] ?? null
  return projected
}

/** Necessary owned rule eligibility; equal keys do not prove a finding or pair admission. */
export function classifyOwnedEligibility(rule, participants) {
  if (!participants.every(isOwnedRecord)) return { eligibility: 'unresolved', differs: [] }
  participants.forEach(validateOwnedRecord)
  const hook = rule === 'redundant-hook'
  if (participants.some((r) => r.type !== (hook ? 'hook' : 'test'))) return { eligibility: 'unresolved', differs: [] }
  const fields = {
    'duplicate-test': ['duplicate'], 'cross-file-duplicate': ['crossFile'],
    'parameterization-candidate': ['parameterization'], 'near-duplicate-test': ['similarity', 'eventSet'],
    'subsumed-test': ['subsumption'], 'duplicate-test-name': ['name'],
    'duplicate-each-row': ['rowClass', 'name', 'duplicate'], 'redundant-hook': ['effects'],
  }[rule]
  if (!fields) return { eligibility: 'unresolved', differs: [] }
  const traceRule = ['parameterization-candidate', 'near-duplicate-test', 'subsumed-test'].includes(rule)
  if (participants.some((r) => (!hook && !r.comparable) || fields.some((field) => r.comparisonKeys[field] === null) || (traceRule && (!r.traceComplete || r.ordinaryEvents < 1)))) return { eligibility: 'not-compared', differs: [] }
  const declaration = (r) => `${r.path}:${r.line}:${r.column}`
  if (rule === 'duplicate-each-row') {
    if (participants.some((r) => r.row === null) || new Set(participants.map(declaration)).size !== 1) return { eligibility: 'not-compared', differs: [] }
  } else if (rule !== 'redundant-hook' && new Set(participants.map(declaration)).size !== participants.length) {
    return { eligibility: 'not-compared', differs: [] }
  }
  const differs = fields.filter((field) => new Set(participants.map((r) => r.comparisonKeys[field])).size > 1).map((field) => `comparisonKeys.${field}`)
  if (hook && new Set(participants.map((r) => r.kind)).size > 1) differs.push('kind')
  return { eligibility: 'eligible', differs }
}
