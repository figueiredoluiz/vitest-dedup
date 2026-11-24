#!/usr/bin/env node
// Writes generated route variants for the loop and registration oracle cases.
//
// The positive is a loop over a `const` array registering a test once per element. Each variant
// applies one route through which that count stops holding, or may stop holding: the array is
// written, the binding redeclared, the loop runs before the declaration initializes, the
// iteration built-in is replaced, or the callback never runs its body. The collection oracle
// judges each file against `fixtures/oracle/loop-routes/expected.json`: Vitest's count and the
// analyzer's either agree or the analyzer refuses the file.
//
// Usage: node scripts/dev/route-variants.mjs [--registration] [out-dir].
// Without --registration it rewrites the loop case.
// `route-variants.test.mjs` fails when the committed files differ from what this writes.
import { mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

/** The case directory, relative to the checkout. */
export const CASE_DIR = 'fixtures/oracle/loop-routes'

/** The registration-value case directory, relative to the checkout. */
export const REGISTRATION_CASE_DIR = 'fixtures/oracle/registration-routes'

const BODY = "test('loop', () => {\n    expect(1).toBe(1)\n  })"
const DECL = 'const rows = [101, 102]'

/** Each iteration form over `rows`, with the built-in it calls. */
const FORMS = {
  forof: { loop: (body) => `for (const row of rows) {\n  ${body}\n}`, builtin: 'Symbol.iterator' },
  foreach: { loop: (body) => `rows.forEach(() => {\n  ${body}\n})`, builtin: "'forEach'" },
  map: { loop: (body) => `rows.map(() => {\n  ${body}\n})`, builtin: "'map'" },
}

/** `text` indented by `depth` levels after its first line. */
const indent = (text, depth) => text.split('\n').join(`\n${'  '.repeat(depth)}`)

/**
 * A replacement of `builtin` that iterates one element for an array holding `rows`'s values and the
 * original for any other. It never names `rows`: a read of it that is not an iteration would refuse
 * the loop on its own, and the variant would no longer test the replacement.
 */
const replaced = (builtin) =>
  `const original = Array.prototype[${builtin}]\n` +
  `/** Iterates one element for an array holding [101, 102], the original otherwise. */\n` +
  `function narrowed(this: unknown[], ...args: unknown[]) {\n` +
  `  const ours = this.length === 2 && this[0] === 101 && this[1] === 102\n` +
  `  return Reflect.apply(original, ours ? [101] : this, args)\n}`

/**
 * The routes, each with the forms it applies to and the file's statements for a form. Every
 * route is one edit away from `control`.
 */
const ROUTES = {
  control: { forms: ['forof', 'foreach', 'map'], body: (f) => [DECL, f.loop(BODY)] },
  push: { forms: ['forof', 'foreach'], body: (f) => [DECL, 'rows.push(3)', f.loop(BODY)] },
  'index-write': { forms: ['forof'], body: (f) => [DECL, 'rows[2] = 3', f.loop(BODY)] },
  'alias-mutates': { forms: ['forof'], body: (f) => [DECL, 'const alias = rows\nalias.pop()', f.loop(BODY)] },
  'let-reassigned': { forms: ['forof'], body: (f) => ['let rows = [101, 102]', 'rows = [101]', f.loop(BODY)] },
  'var-redeclared': { forms: ['forof'], body: (f) => ['var rows = [101, 102]', 'var rows = [101]', f.loop(BODY)] },
  'tdz-try': { forms: ['forof', 'foreach'], body: (f) => [`try {\n  ${indent(f.loop(BODY), 1)}\n} catch {}`, DECL] },
  'tdz-switch': {
    forms: ['forof'],
    body: (f) => ['const k: number = 1', `switch (k) {\n  case 0:\n    ${DECL}\n    break\n  case 1:\n    try {\n      ${indent(f.loop(BODY), 3)}\n    } catch {}\n}`],
  },
  'tdz-class-static': { forms: ['forof'], body: (f) => [`class Register {\n  static {\n    try {\n      ${indent(f.loop(BODY), 3)}\n    } catch {}\n  }\n}`, DECL] },
  'builtin-assigned': { forms: ['forof', 'foreach', 'map'], body: (f) => [DECL, `${replaced(f.builtin)}\nArray.prototype[${f.builtin}] = narrowed as never`, f.loop(BODY)] },
  'builtin-defined': { forms: ['forof', 'foreach'], body: (f) => [DECL, `${replaced(f.builtin)}\nObject.defineProperty(Array.prototype, ${f.builtin}, { value: narrowed })`, f.loop(BODY)] },
  'builtin-destructured': { forms: ['forof', 'foreach'], body: (f) => [DECL, `${replaced(f.builtin)}\nconst { prototype } = Array\nprototype[${f.builtin}] = narrowed as never`, f.loop(BODY)] },
  'generator-callback': { forms: ['foreach', 'map'], body: (f) => [DECL, f.loop(BODY).replace('() => {', 'function* () {')] },
  'async-generator-callback': { forms: ['foreach'], body: (f) => [DECL, f.loop(BODY).replace('() => {', 'async function* () {')] },
}

/** The variant files, by name, in a stable order. */
export function variants() {
  const files = {}
  for (const [route, { forms, body }] of Object.entries(ROUTES)) {
    for (const form of forms) {
      const statements = body(FORMS[form])
      files[`${form}--${route}.test.ts`] =
        `// Generated by scripts/dev/route-variants.mjs (route ${route}, form ${form}); do not edit.\n` +
        `import { test, expect } from 'vitest'\n\n${statements.join('\n\n')}\n\ntest('twin', () => {\n  expect(1).toBe(1)\n})\n`
    }
  }
  return files
}

const REGISTRATION_FORMS = {
  direct: { source: "import { test } from 'vitest'", call: 'test' },
  'plain-alias': { source: "import { plain as register } from './values'", call: 'register' },
  'exported-alias': { source: "import { exported as register } from './values'", call: 'register' },
  namespace: { source: "import * as values from './values'", call: 'values.exported' },
  'nested-result': { source: "import { nested as register } from './values'", call: 'register' },
  closed: {
    source: "import { test } from 'vitest'\nimport { closed as register } from './values'",
    call: 'register',
    control: "test('closed control', () => expect('control').toBe('control'))\n",
  },
  opaque: { source: "import { opaque as register } from './values'", call: 'register' },
}

/** Render one test pair through each registration form, with a direct test in the closed case. */
export function registrationVariants() {
  return Object.fromEntries(Object.entries(REGISTRATION_FORMS).map(([form, { source, call, control = '' }]) => [
    `${form}.test.ts`,
    `// Generated by scripts/dev/route-variants.mjs --registration; do not edit.\n` +
      `import { expect } from 'vitest'\n${source}\n\n` +
      `${call}('${form} left', () => expect('${form}').toBe('${form}'))\n` +
      `${call}('${form} right', () => expect('${form}').toBe('${form}'))\n` +
      control,
  ]))
}

/** Rewrites the variant files under `out`, leaving `expected.json` and any other file alone. */
export function write(out, files = variants()) {
  mkdirSync(out, { recursive: true })
  for (const name of readdirSync(out)) if (name.endsWith('.test.ts')) rmSync(join(out, name))
  for (const [name, text] of Object.entries(files)) writeFileSync(join(out, name), text)
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const registration = process.argv[2] === '--registration'
  const files = registration ? registrationVariants() : variants()
  const out = resolve(process.argv[registration ? 3 : 2] ?? join(ROOT, registration ? REGISTRATION_CASE_DIR : CASE_DIR))
  write(out, files)
  console.log(`${Object.keys(files).length} variants in ${out}`)
}
