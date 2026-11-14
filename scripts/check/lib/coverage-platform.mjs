// Native proof execution is measured by macOS CI; shared developer gates run on every host.
const MACOS_ONLY = new Set(['scripts/dev/revert-proof.mjs', 'scripts/dev/proof-command.mjs'])

/** Partition unchanged coverage floors by the host capable of exercising their native paths. */
export function coverageOnHost(floors, platform = process.platform) {
  const measured = {}
  const deferred = {}
  for (const [module, floor] of Object.entries(floors)) {
    const target = platform !== 'darwin' && MACOS_ONLY.has(module) ? deferred : measured
    target[module] = floor
  }
  return { measured, deferred }
}

/**
 * Name the integration-test targets a Cargo test run did not report running.
 * Cargo prints `Running tests/<name>.rs (...)` on stderr for each target it executes; a target
 * missing from that list did not run, whatever the summed test count says.
 * @param {string} log Combined stdout and stderr of the test run.
 * @param {string[]} targets Repository-relative `tests/*.rs` paths.
 * @returns {string[]} The targets with no `Running` line, in the order given.
 */
export function missingTestTargets(log, targets) {
  // Colour codes are stripped first: `CARGO_TERM_COLOR=always` wraps the word `Running` in them.
  const plain = log.replace(/\x1b\[[0-9;]*m/g, '')
  const ran = new Set([...plain.matchAll(/^\s*Running (tests[\\/]\S+\.rs)\b/gm)].map(match => match[1].replaceAll('\\', '/')))
  return targets.filter(target => !ran.has(target))
}

/**
 * The environment for a test run whose `Running` lines `missingTestTargets` reads.
 * Quiet mode omits those lines and verbose mode names the binary instead of the source file, while
 * every target still runs; the variables also override `term.quiet` and `term.verbose` in Cargo config.
 * @param {NodeJS.ProcessEnv} env The inherited environment.
 * @returns {NodeJS.ProcessEnv} A copy with Cargo's default status output forced.
 */
export function targetReportingEnv(env = process.env) {
  return { ...env, CARGO_TERM_QUIET: 'false', CARGO_TERM_VERBOSE: 'false' }
}
