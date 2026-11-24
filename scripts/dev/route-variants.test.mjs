// The committed route cases are what the generator writes, and every variant is pinned.
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { expectedProblems } from '../check/lib/oracle-cases.mjs'
import { CASE_DIR, REGISTRATION_CASE_DIR, registrationVariants, variants, write } from './route-variants.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const dir = join(root, CASE_DIR)

/** Checks the exact set and source of generated test files in one owned oracle case. */
function assertGenerated(caseDir, generated) {
  const committed = readdirSync(caseDir).filter((name) => name.endsWith('.test.ts')).sort()
  assert.deepEqual(committed, Object.keys(generated).sort(), caseDir)
  for (const name of committed) {
    const source = readFileSync(join(caseDir, name), 'utf8').replaceAll('\r\n', '\n')
    assert.equal(source, generated[name], name)
  }
}

test('the committed variants are the generated ones', () => {
  assertGenerated(dir, variants())
})

test('generated registration forms have exact oracle pins and a directly comparable control', () => {
  const caseDir = join(root, REGISTRATION_CASE_DIR)
  const generated = registrationVariants()
  assertGenerated(caseDir, generated)
  const expected = JSON.parse(readFileSync(join(caseDir, 'expected.json'), 'utf8'))
  assert.deepEqual(expectedProblems(expected, REGISTRATION_CASE_DIR), [])
  const { files } = expected
  assert.deepEqual(Object.keys(files).sort(), Object.keys(generated).sort())
  assert.deepEqual(files['direct.test.ts'], { tasks: 2, wrapperSites: [], next: {} })
})

test('every variant is declared in expected.json, and each control is compared unrefused', () => {
  const { files } = JSON.parse(readFileSync(join(dir, 'expected.json'), 'utf8'))
  const names = Object.keys(variants())
  assert.ok(names.length >= 20, `${names.length} variants`)
  assert.deepEqual(Object.keys(files).sort(), [...names].sort())
  const controls = names.filter((name) => name.includes('--control.'))
  assert.equal(controls.length, 3)
  for (const name of controls) assert.deepEqual(files[name].next, {}, name)
})

test('write replaces stale variants and keeps other files', () => {
  const out = mkdtempSync(join(tmpdir(), 'route-variants-'))
  try {
    writeFileSync(join(out, 'stale.test.ts'), '')
    writeFileSync(join(out, 'expected.json'), '{}')
    write(out)
    const names = readdirSync(out).sort()
    assert.deepEqual(names, [...Object.keys(variants()), 'expected.json'].sort())
    assert.equal(readFileSync(join(out, 'expected.json'), 'utf8'), '{}')
  } finally {
    rmSync(out, { recursive: true, force: true })
  }
})

test('each variant is one route applied to its form control', () => {
  const generated = variants()
  for (const [name, text] of Object.entries(generated)) {
    const [form] = name.split('--')
    if (name === `${form}--control.test.ts`) continue
    assert.notEqual(text.split('\n').slice(1).join('\n'), generated[`${form}--control.test.ts`].split('\n').slice(1).join('\n'), name)
  }
})
