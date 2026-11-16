// The committed loop-route oracle case is what the generator writes, and every variant is pinned.
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { CASE_DIR, variants, write } from './route-variants.mjs'

const dir = join(resolve(dirname(fileURLToPath(import.meta.url)), '..', '..'), CASE_DIR)

test('the committed variants are the generated ones', () => {
  const generated = variants()
  const committed = readdirSync(dir).filter((name) => name.endsWith('.test.ts')).sort()
  assert.deepEqual(committed, Object.keys(generated).sort(), 'run node scripts/dev/route-variants.mjs')
  for (const name of committed) assert.equal(readFileSync(join(dir, name), 'utf8'), generated[name], name)
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
