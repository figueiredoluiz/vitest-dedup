import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { test } from 'node:test'

const gate = fileURLToPath(new URL('../check-memory-scaling.mjs', import.meta.url))

/** Drive the real CLI with bounded measurement fixtures instead of generating large corpora. */
function runScaling(scenario) {
  const root = mkdtempSync(join(tmpdir(), 'vitest-dedup-scaling-test-'))
  try {
    const events = join(root, 'events.jsonl')
    const loader = join(root, 'loader.mjs')
    const fixture = `
      import { appendFileSync } from 'node:fs';
      const scenario = ${JSON.stringify(scenario)};
      const event = (value) => appendFileSync(${JSON.stringify(events)}, JSON.stringify(value) + '\\n');
      export const CORPUS_SCALES = Object.fromEntries([1000, 10000, 100000].map(tests =>
        [tests, { tests, analyzedFiles: 1, digest: 'owned-' + tests }]));
      export const corpusRoot = (_base, scale) => String(scale);
      export function generateCorpus({ tests }) {
        event(['generate', tests]);
        return { digest: scenario === 'digest' && tests === 10000 ? 'changed' : 'owned-' + tests };
      }
      export const parseGateArguments = () => ({ binary: ${JSON.stringify(join(root, 'no-analyzer'))}, keep: false });
      export const mb = value => String(value);
      export function measureMemory(_binary, root) {
        const tests = Number(root);
        event(['measure', tests]);
        if (scenario === 'throw' && tests === 10000) throw new Error('owned measurement failure');
        const ir = tests > 1000 && scenario === 'growth' ? 600
          : tests > 1000 && scenario === 'growth-boundary' ? 550 : 500;
        const spine = scenario === 'height' ? 1951 : scenario === 'height-boundary' ? 1950
          : scenario === 'spine' && tests === 10000 ? 1000 : 100;
        return { elapsedMs: 1, record: {
          testsAnalyzed: tests, filesAnalyzed: 1, declarationsRetained: tests,
          retainedBytes: (ir + spine) * tests, bytesPerTest: ir + spine,
          fields: { spine: spine * tests }, peakRssBytes: tests === 1000 ? null : 1e12,
          problems: scenario === 'work' && tests === 10000 ? ['owned work-count refusal'] : [],
        }};
      }
      export const workPerformedProblems = record => record.problems;
      export function discardCorpus() { event(['cleanup']); return 'owned fixture cleaned'; }
    `
    const fixtureUrl = `data:text/javascript,${encodeURIComponent(fixture)}`
    writeFileSync(loader, `
      export async function resolve(specifier, context, nextResolve) {
        const resolved = await nextResolve(specifier, context);
        if (resolved.url.endsWith('/lib/memory-corpus.mjs') || resolved.url.endsWith('/lib/memory-record.mjs'))
          return { url: ${JSON.stringify(fixtureUrl)}, shortCircuit: true };
        if (resolved.url === ${JSON.stringify(pathToFileURL(gate).href)} || resolved.url.startsWith('node:'))
          return resolved;
        throw new Error('Unexpected scaling fixture dependency: ' + resolved.url);
      }
    `)
    const preload = `import { register } from 'node:module'; register(${JSON.stringify(pathToFileURL(loader).href)});`
    const result = spawnSync(process.execPath, [
      '--import', `data:text/javascript,${encodeURIComponent(preload)}`, gate,
    ], { cwd: root, encoding: 'utf8', timeout: 10_000, killSignal: 'SIGKILL', maxBuffer: 1 << 20 })
    assert.ifError(result.error)
    assert.equal(result.signal, null, result.stderr)
    return {
      ...result,
      events: readFileSync(events, 'utf8').trim().split('\n').map(line => JSON.parse(line)),
    }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

for (const [scenario, exponent] of [
  ['linear', 1], ['spine', 1], ['height-boundary', 1], ['growth-boundary', 1.0174],
]) {
  test(`scaling accepts ${scenario} retention and reports a target separately from enforced bounds`, () => {
    const result = runScaling(scenario)
    assert.equal(result.status, 0, result.stderr)
    assert.equal(Number(/fitted exponent\s+(\S+)/.exec(result.stdout)?.[1]), exponent)
    assert.match(result.stdout, /2 GB target/)
    assert.deepEqual(result.events, [
      ['generate', 1000], ['measure', 1000], ['generate', 10000], ['measure', 10000],
      ['generate', 100000], ['measure', 100000], ['cleanup'],
    ])
  })
}

for (const [scenario, diagnostic] of [
  ['growth', /IR bytes per test grew 1\.200x/],
  ['height', /ceiling of 2450/],
  ['work', /owned work-count refusal/],
  ['throw', /owned measurement failure/],
  ['digest', /10000-test corpus has digest changed/],
]) {
  test(`scaling rejects ${scenario} failures and delegates cleanup once`, () => {
    const result = runScaling(scenario)
    assert.equal(result.status, 1, `${result.stdout}\n${result.stderr}`)
    assert.match(result.stderr, diagnostic)
    assert.deepEqual(result.events.filter(event => event[0] === 'cleanup'), [['cleanup']])
    assert.equal(result.events.at(-1)[0], 'cleanup')
    const measured = result.events.filter(event => event[0] === 'measure').map(event => event[1])
    assert.deepEqual(measured, scenario === 'digest' ? [1000] : scenario === 'throw'
      ? [1000, 10000] : [1000, 10000, 100000])
    if (['work', 'throw', 'digest'].includes(scenario)) assert.doesNotMatch(result.stdout, /fitted exponent/)
    if (scenario === 'growth') assert.doesNotMatch(result.stderr, /ceiling of 2450/)
    if (scenario === 'height') assert.doesNotMatch(result.stderr, /IR bytes per test grew/)
  })
}
