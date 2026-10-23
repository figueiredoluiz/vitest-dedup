// The release-notes section states what a build leaves out of comparison.
import assert from 'node:assert/strict'
import test from 'node:test'

import { renderSection } from './release-comparison.mjs'

/** A comparison with the classes at zero and the given exclusion totals. */
const comparison = (excluded, cases = []) => ({
  totals: {
    detector: { retained: 5, 'newly-satisfied': 0, corrected: 0, regressed: 0, 'new-forbidden': 0 },
    newUnclaimed: 0,
    extractionRegressed: 0,
    ...(excluded === undefined ? {} : { excluded }),
  },
  unavailableCases: 0,
  labelChangedCases: [],
  cases,
})

const render = (excluded, cases) => renderSection({ comparison: comparison(excluded, cases), version: '0.8.0', previousTag: 'v0.7.0', cases: 9 })

test('the section states the tests left out of comparison and the cases that leave more out', () => {
  const text = render({ baseline: 27, candidate: 23, measuredCases: 9, more: [], fewer: 3 })
  assert.match(text, /\| \*\*Cases that leave more tests out of comparison than v0\.7\.0\*\* \| 0 \|/)
  assert.match(text, /\| Tests left out of comparison, over the 9 case\(s\) measured in both builds \(v0\.7\.0 to v0\.8\.0\) \| 27 to 23 \|/)
  const worse = render({ baseline: 4, candidate: 9, measuredCases: 9, more: [{ name: 'w', was: 4, is: 9 }], fewer: 0 })
  assert.match(worse, /leave more tests out of comparison than v0\.7\.0\*\* \| 1 \|/)
  assert.match(worse, /\| 4 to 9 \|/)
})

test('a comparison with nothing measured, or from an older evaluator, reads as not measured, never as zero', () => {
  for (const excluded of [{ baseline: 0, candidate: 0, measuredCases: 0, more: [], fewer: 0 }, undefined]) {
    const text = render(excluded)
    assert.match(text, /measured in both builds \(v0\.7\.0 to v0\.8\.0\) \| not measured \|/)
    assert.doesNotMatch(text, /\| 0 to 0 \|/)
  }
})

test('a case that leaves more tests out is named in the section', () => {
  const rose = { name: 'wide-case', pending: null, labelChange: null, newUnclaimed: [], extraction: 'retained', expectations: [], excluded: { was: 1, is: 5, rose: true } }
  const fine = { ...rose, name: 'fine-case', excluded: { was: 1, is: 1, rose: false } }
  const text = render({ baseline: 2, candidate: 6, measuredCases: 2, more: [{ name: 'wide-case', was: 1, is: 5 }], fewer: 0 }, [rose, fine])
  assert.match(text, /Cases with a regression \(1\):/)
  assert.match(text, /- `wide-case`/)
  assert.doesNotMatch(text, /fine-case/)
})
