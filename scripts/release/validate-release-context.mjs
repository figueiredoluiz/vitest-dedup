#!/usr/bin/env node
// Only an explicit dispatch on an existing tag at the checked-out commit may publish.
import { pathToFileURL } from 'node:url'

const shaPattern = /^[0-9a-f]{40}$/

export function validateReleaseContext({ publish, tag, ref, eventSha, releaseSha, tagSha }) {
  if (!shaPattern.test(eventSha) || !shaPattern.test(releaseSha) || eventSha !== releaseSha) {
    throw new Error('the checked-out commit must equal the workflow event commit')
  }
  if (!publish) return

  if (ref !== `refs/tags/${tag}`) {
    throw new Error('publishing requires a dispatch from the matching release tag')
  }
  if (!shaPattern.test(tagSha) || tagSha !== releaseSha) {
    throw new Error('the release tag must exist at the checked-out commit')
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    validateReleaseContext({
      publish: process.env.PUBLISH_RELEASE === 'true',
      tag: process.env.RELEASE_TAG,
      ref: process.env.EVENT_REF,
      eventSha: process.env.EVENT_SHA,
      releaseSha: process.env.RELEASE_SHA,
      tagSha: process.env.TAG_SHA,
    })
    console.log('Release context verified.')
  } catch (error) {
    console.error(`Release context rejected: ${error.message}`)
    process.exitCode = 1
  }
}
