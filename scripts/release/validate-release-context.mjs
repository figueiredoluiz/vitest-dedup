#!/usr/bin/env node
// Only an explicit dispatch on an existing tag at the checked-out commit may publish.
import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

const shaPattern = /^[0-9a-f]{40}$/

/** Rejects publication unless the event, checkout, and exact release tag identify one commit. */
export function validateReleaseContext({ publish, npmOnly = false, tag, ref, eventSha, releaseSha, tagSha }) {
  if (!shaPattern.test(eventSha) || !shaPattern.test(releaseSha) || eventSha !== releaseSha) {
    throw new Error('the checked-out commit must equal the workflow event commit')
  }
  if (npmOnly && !publish) {
    throw new Error('npm-only recovery requires publishing to be enabled')
  }
  if (!publish) return

  if (ref !== `refs/tags/${tag}`) {
    throw new Error('publishing requires a dispatch from the matching release tag')
  }
  if (!shaPattern.test(tagSha) || tagSha !== releaseSha) {
    throw new Error('the release tag must exist at the checked-out commit')
  }
}

/** Reads an exact live tag from `git ls-remote`, preferring an annotated tag's peeled commit. */
export function remoteTagCommit(output, tag) {
  const ref = `refs/tags/${tag}`
  const entries = new Map(output.trim().split('\n').map((line) => {
    const [sha, name] = line.split('\t')
    return [name, sha]
  }))
  const tagObject = entries.get(ref)
  const peeled = entries.get(`${ref}^{}`)
  if (!shaPattern.test(tagObject) || (peeled !== undefined && !shaPattern.test(peeled))) {
    throw new Error('the release tag must exist at the remote')
  }
  return peeled ?? tagObject
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const tag = process.env.RELEASE_TAG
    let tagSha = process.env.TAG_SHA
    if (process.argv[2] === '--remote') {
      const ref = `refs/tags/${tag}`
      const refs = execFileSync('git', ['ls-remote', '--exit-code', 'origin', ref, `${ref}^{}`], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      tagSha = remoteTagCommit(refs, tag)
    }
    validateReleaseContext({
      publish: process.env.PUBLISH_RELEASE === 'true',
      npmOnly: process.env.PUBLISH_NPM_ONLY === 'true',
      tag,
      ref: process.env.EVENT_REF,
      eventSha: process.env.EVENT_SHA,
      releaseSha: process.env.RELEASE_SHA,
      tagSha,
    })
    console.log('Release context verified.')
  } catch (error) {
    console.error(`Release context rejected: ${error.message}`)
    process.exitCode = 1
  }
}
