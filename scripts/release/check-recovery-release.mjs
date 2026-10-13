#!/usr/bin/env node
// npm-only recovery must reuse an existing published release for the exact live tag.
import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

/** Rejects a missing, draft, or differently tagged release before npm recovery can publish. */
export function validateRecoveryRelease(release, tag) {
  if (!/^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?$/.test(tag) ||
      release?.tagName !== tag || release.isDraft !== false) {
    throw new Error('npm recovery requires a matching published GitHub Release')
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const tag = process.env.RELEASE_TAG
    const repo = process.env.GH_REPO
    if (!repo) throw new Error('GH_REPO is required')
    const output = execFileSync('gh',
      ['release', 'view', tag, '--repo', repo, '--json', 'tagName,isDraft'],
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
    validateRecoveryRelease(JSON.parse(output), tag)
    console.log(`Verified existing GitHub Release ${tag}.`)
  } catch (error) {
    console.error(`npm recovery release check failed: ${error.message}`)
    process.exitCode = 1
  }
}
