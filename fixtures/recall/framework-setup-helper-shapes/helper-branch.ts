// Branches on the environment.
/** Setup helper `setupBranch` of this case, called from its test file(s). */
export function setupBranch() {
  if (process.env.APP_MODE === 'ci') {
    console.info('ci mode')
  }
}
