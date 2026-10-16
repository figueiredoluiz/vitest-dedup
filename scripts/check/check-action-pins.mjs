#!/usr/bin/env node
// Remote workflow code must be immutable; parse YAML so quoted and flow-style steps are checked.
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { isMap, isSeq, parseDocument } from 'yaml'
import { runAsGate } from './lib/gate-main.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const REMOTE = /^[\w.-]+\/[\w.-]+(?:\/[\w./-]+)?@[a-f0-9]{40}$/i
const DOCKER = /^docker:\/\/[^\s@]+@sha256:[a-f0-9]{64}$/i

/**
 * Check actual job/step uses fields in every workflow, refusing invalid or empty input.
 * @param {string[]} argv CLI arguments; none are supported.
 * @param {{root?: string}} options Optional fixture root for isolated tests.
 * @returns {{status: number, lines: string[]}} Gate exit status and bounded diagnostics.
 */
export function runCli(argv = [], { root = ROOT } = {}) {
  if (argv.length) return { status: 1, lines: ['Usage: node scripts/check/check-action-pins.mjs'] }
  const directory = join(root, '.github', 'workflows')
  const files = readdirSync(directory).filter(name => /\.ya?ml$/.test(name)).sort()
  const problems = []
  let references = 0
  /** Validate one uses scalar and its human-readable original ref comment. */
  const check = (node, location) => {
    const value = node?.value
    if (typeof value === 'string' && value.startsWith('./') && value.length >= 2) return
    references++
    if (typeof value !== 'string' || !(REMOTE.test(value) || DOCKER.test(value))) {
      problems.push(`${location}: remote uses must select a full commit SHA or Docker sha256 digest`)
    } else if (!node.comment?.trim()) {
      problems.push(`${location}: pinned uses needs a comment identifying the original version/ref`)
    }
  }
  for (const file of files) {
    const document = parseDocument(readFileSync(join(directory, file), 'utf8'), { uniqueKeys: true })
    if (document.errors.length) {
      problems.push(`${file}: invalid YAML`)
      continue
    }
    const jobs = document.get('jobs', true)
    if (!isMap(jobs) || jobs.items.length === 0) {
      problems.push(`${file}: expected non-empty jobs mapping`)
      continue
    }
    for (const { key, value: job } of jobs.items) {
      const location = `${file}:${key.value}`
      if (!isMap(job)) { problems.push(`${location}: expected job mapping`); continue }
      if (job.has('uses')) check(job.get('uses', true), location)
      if (!job.has('steps')) continue
      const steps = job.get('steps', true)
      if (!isSeq(steps)) { problems.push(`${location}: expected steps sequence`); continue }
      for (const [index, step] of steps.items.entries()) {
        if (!isMap(step)) { problems.push(`${location}: expected step mapping`); continue }
        if (step.has('uses')) check(step.get('uses', true), `${location}:step ${index + 1}`)
      }
    }
  }
  if (!files.length || !references) problems.push('action pin check scanned no workflows or remote references')
  return {
    status: problems.length ? 1 : 0,
    lines: [`action pins: ${files.length} workflows, ${references} remote references`, ...problems],
  }
}

runAsGate(import.meta.url, runCli, 'action pins')
