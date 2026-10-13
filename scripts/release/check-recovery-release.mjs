#!/usr/bin/env node
// npm-only recovery must reuse an existing published release for the exact tagged commit.
import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

/** Rejects a missing, draft, or differently targeted release before npm recovery can publish. */
export function validateRecoveryRelease(release, tag, sha) {
  if (!/^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?$/.test(tag) ||
      !/^[0-9a-f]{40}$/.test(sha) ||
      release?.tagName !== tag || release.targetCommitish !== sha || release.isDraft !== false) {
    throw new Error('npm recovery requires a matching published GitHub Release at the event commit')
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const tag = process.env.RELEASE_TAG
    const sha = process.env.RELEASE_SHA
    const repo = process.env.GH_REPO
    if (!repo) throw new Error('GH_REPO is required')
    const output = execFileSync('gh',
      ['release', 'view', tag, '--repo', repo, '--json', 'tagName,targetCommitish,isDraft'],
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
    validateRecoveryRelease(JSON.parse(output), tag, sha)
    console.log(`Verified existing GitHub Release ${tag} at ${sha}.`)
  } catch (error) {
    console.error(`npm recovery release check failed: ${error.message}`)
    process.exitCode = 1
  }
}
