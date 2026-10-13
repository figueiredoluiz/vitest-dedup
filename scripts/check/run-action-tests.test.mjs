import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const runner = fileURLToPath(new URL('./run-action-tests.mjs', import.meta.url))
const root = fileURLToPath(new URL('../../', import.meta.url))

test('negative Action fixtures stay visible without creating live workflow annotations', () => {
  const env = { ...process.env }
  delete env.NODE_TEST_CONTEXT
  const result = spawnSync(process.execPath, [runner], {
    cwd: root,
    env,
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
    timeout: 30_000,
  })
  assert.equal(result.status, 0, result.stderr)

  const lines = result.stdout.split(/\r?\n/)
  const marker = lines[0]?.match(/^::stop-commands::([0-9a-f-]{36})$/)?.[1]
  assert.ok(marker, 'captured test output must pause GitHub workflow commands')
  const resume = lines.indexOf(`::${marker}::`)
  assert.ok(resume > 0, 'workflow commands must resume after captured test output')

  const diagnostics = lines.slice(1, resume)
  assert.ok(diagnostics.some((line) => line.startsWith('::error::')))
  assert.ok(diagnostics.some((line) => line.startsWith('::warning::')))
  assert.ok(lines.slice(resume + 1).some((line) => /^ok: \d+ action tests ran and passed\.$/.test(line)))
  assert.ok(!lines.slice(resume + 1).some((line) => /^::(?:error|warning)::/.test(line)))
})
