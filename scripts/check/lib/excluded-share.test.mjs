// Unit tests for the exclusion figures a baseline pins.
import assert from 'node:assert/strict'
import test from 'node:test'

import { compareExcluded, excludedFigures, exclusionsDrifted, recordable, shareText } from './excluded-share.mjs'

/** A summary the way the analyzer writes it. */
const summary = (over = {}) => ({
  testsAnalyzed: 200,
  filesAnalyzed: 40,
  exclusions: {
    files: 6,
    tests: 50,
    omitted: 0,
    reasons: [
      { reason: 'helper-setup-effects', files: 4, tests: 40 },
      { reason: 'unresolved-context', files: 2, tests: 10 },
    ],
  },
  ...over,
})

test('the figures are read with their share and recorded without the run totals', () => {
  const figures = excludedFigures(summary())
  assert.equal(figures.tests, 50)
  assert.equal(shareText(figures), '25.0%')
  assert.deepEqual(recordable(figures), {
    tests: 50,
    files: 6,
    reasons: { 'helper-setup-effects': { files: 4, tests: 40 }, 'unresolved-context': { files: 2, tests: 10 } },
  })
})

test('a summary that does not state its exclusions, or states them inconsistently, is refused', () => {
  const refused = (value, pattern) => assert.throws(() => excludedFigures(value), pattern)
  refused(summary({ exclusions: undefined }), /no `exclusions`/)
  refused(summary({ exclusions: { files: 1, tests: 1 } }), /malformed/)
  refused(summary({ exclusions: { files: 1, tests: -1, reasons: [] } }), /malformed/)
  refused(summary({ testsAnalyzed: 0 }), /analyzed no tests/)
  refused(summary({ filesAnalyzed: 0 }), /analyzed no tests or no files/)
  refused(summary({ testsAnalyzed: undefined }), /does not state how many/)
  refused(summary({ testsAnalyzed: 10 }), /50 test\(s\) excluded out of 10/)
  refused(summary({ exclusions: { files: 6, tests: 50, reasons: [{ reason: 'a', files: 7, tests: 1 }] } }), /do not add up to the totals/)
  refused(summary({ exclusions: { files: 6, tests: 50, reasons: [] } }), /do not add up to the totals/)
  refused(summary({ exclusions: { files: 6, tests: 50, reasons: [{ reason: 'a', files: 6, tests: 49 }] } }), /do not add up to the totals/)
  refused(summary({ exclusions: { files: 6, tests: 50, reasons: [{ reason: 'a', files: 1, tests: 1 }, { reason: 'a', files: 1, tests: 1 }] } }), /repeated/)
  refused(null, /no `exclusions`/)
})

test('a rise is reported as a regression, a fall as a gain, and an unchanged run as neither', () => {
  const base = recordable(excludedFigures(summary()))
  assert.deepEqual(compareExcluded(base, excludedFigures(summary())), { rose: [], fell: [], unrecorded: false })
  const worse = excludedFigures(
    summary({
      exclusions: {
        files: 9,
        tests: 80,
        reasons: [
          { reason: 'helper-setup-effects', files: 7, tests: 70 },
          { reason: 'unresolved-context', files: 2, tests: 10 },
        ],
      },
    }),
  )
  const rise = compareExcluded(base, worse)
  assert.deepEqual(rise.rose, ['tests excluded 50 -> 80', 'files excluded 6 -> 9', 'helper-setup-effects tests 40 -> 70', 'helper-setup-effects files 4 -> 7'])
  assert.deepEqual(rise.fell, [])
  const better = excludedFigures(
    summary({ exclusions: { files: 2, tests: 10, reasons: [{ reason: 'unresolved-context', files: 2, tests: 10 }] } }),
  )
  const gain = compareExcluded(base, better)
  assert.deepEqual(gain.rose, [])
  assert.deepEqual(gain.fell, ['tests excluded 50 -> 10', 'files excluded 6 -> 2', 'helper-setup-effects tests 40 -> 0', 'helper-setup-effects files 4 -> 0'])
})

test('a reason that appears or disappears is a change in that reason, not a hidden one', () => {
  const base = recordable(excludedFigures(summary()))
  const appeared = excludedFigures(
    summary({
      exclusions: {
        files: 7,
        tests: 53,
        reasons: [...summary().exclusions.reasons, { reason: 'runner-mocked', files: 1, tests: 3 }],
      },
    }),
  )
  assert.deepEqual(compareExcluded(base, appeared).rose, ['tests excluded 50 -> 53', 'files excluded 6 -> 7', 'runner-mocked tests 0 -> 3', 'runner-mocked files 0 -> 1'])
})

test('a baseline recorded before the figures existed is reported as unrecorded, not as a change', () => {
  assert.deepEqual(compareExcluded(undefined, excludedFigures(summary())), { rose: [], fell: [], unrecorded: true })
  assert.deepEqual(compareExcluded(null, excludedFigures(summary())), { rose: [], fell: [], unrecorded: true })
})

test('a damaged recorded baseline is refused rather than read as unchanged', () => {
  const now = excludedFigures(summary())
  const good = recordable(now)
  const tooMany = { tests: 5, files: 5, reasons: { a: { files: 1, tests: 9 } } }
  const tooFew = { tests: 50, files: 6, reasons: { a: { files: 1, tests: 9 } } }
  for (const damaged of [{}, { ...good, tests: '50' }, { ...good, files: undefined }, { ...good, reasons: null }, { ...good, reasons: [] }, { tests: 0, files: 0, reasons: [] }, { ...good, reasons: { a: { files: 1 } } }, { tests: 1, files: 1 }, tooMany, tooFew]) {
    assert.throws(() => compareExcluded(damaged, now), /malformed|not add up/)
  }
})

test('any change in either direction is drift, and an unchanged run is not', () => {
  const base = recordable(excludedFigures(summary()))
  assert.equal(exclusionsDrifted(compareExcluded(base, excludedFigures(summary()))), false)
  assert.equal(exclusionsDrifted({ rose: ['x'], fell: [] }), true)
  assert.equal(exclusionsDrifted({ rose: [], fell: ['x'] }), true)
  assert.equal(exclusionsDrifted(compareExcluded(undefined, excludedFigures(summary()))), false, 'an unrecorded baseline is a note, not drift')
})
