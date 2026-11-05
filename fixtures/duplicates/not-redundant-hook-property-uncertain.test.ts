// Twin of redundant-hook-property-twin.test.ts with a delete on the tracked property
// reachable somewhere in the module: the write set can no longer prove the property keeps one
// value across the lifecycle, so the pair must stay silent. The delete sits in a never-called
// function because a statement inside a runtime callback would independently withhold the
// scheduling proof -- the isolated variable is the path's value history, not scheduling. Named
// weakening: remove the delete and the pair fires exactly as in the twin.
// @vitest-dedup:extracted 1
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:participant L20 "beforeEach"
// @vitest-dedup:participant L24 "beforeEach"
// @vitest-dedup:isolates not-isolable the two hooks are byte-identical, so the isolated variable is the tracked path's write history, which no participant fingerprint carries; the guard is the corpus gate's silence assertion plus the named weakening in the header
import { describe, test, expect, beforeEach } from 'vitest'

const audit = { mode: '' }

function route() {
  delete audit.mode
}

beforeEach(() => {
  audit.mode = 'primary'
})

beforeEach(() => {
  audit.mode = 'primary'
})

test('reads the mode', () => {
  expect(audit.mode).toBe('primary')
})
