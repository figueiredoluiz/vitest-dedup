// Stands in for the analyzer binary under the existing checkers. Each invocation runs the baseline
// binary and then the candidate with the original arguments, records any difference, and
// reproduces the candidate's result so the checker sees only it. Every file a run writes (each file
// reporter's report and an updated `--baseline`) is snapshotted before each
// run and restored before the candidate runs, so both start from one state and each run is compared
// on what it created, rewrote or deleted there. Standard input is not forwarded to either binary:
// no gate or test pipes input to the analyzer, and a caller that started to would need this changed.
import { spawnSync } from 'node:child_process'
import { compareRuns, pairRuns, versionReplacements, writeLocations, writeRecord } from './output-equivalence.mjs'

const {
  VITEST_DEDUP_EQUIVALENCE_BASELINE: baseline,
  VITEST_DEDUP_EQUIVALENCE_CANDIDATE: candidate,
  VITEST_DEDUP_EQUIVALENCE_LOG: log,
  VITEST_DEDUP_EQUIVALENCE_SUITE: suite,
  VITEST_DEDUP_EQUIVALENCE_VERSIONS: versions,
} = process.env
if (!baseline || !candidate || !log || !suite) {
  process.stderr.write('equivalence shim: run it through scripts/check/check-output-equivalence.mjs\n')
  process.exit(2)
}

const args = process.argv.slice(2)
// Streams stay bytes: they are compared exactly when not valid UTF-8, and forwarded unchanged.
const options = { maxBuffer: 512 * 1024 * 1024 }

/** One run of `binary` with the original arguments; `pairRuns` adds the files it wrote. */
function execute(binary) {
  const run = spawnSync(binary, args, options)
  if (run.error) throw run.error
  return { status: run.status, signal: run.signal, stdout: run.stdout, stderr: run.stderr, files: null }
}

/** Runs both binaries and returns what differed; throws when the comparison itself cannot be made. */
function observe() {
  // Reporters and `--output` are command-line only, so the locations follow from the arguments alone.
  const locations = writeLocations(args, process.cwd())
  const [baselineRun, candidateRun] = pairRuns(
    locations,
    () => execute(baseline),
    () => execute(candidate),
  )
  const differences = compareRuns(baselineRun, candidateRun, {
    candidate: JSON.parse(versions || '[]').flatMap(versionReplacements),
  })
  return { candidateRun, differences }
}

let observed
try {
  observed = observe()
} catch (error) {
  // A refused location or a failed restore is not a pass: a test asserting the exit code the shim
  // happens to return would accept it, so it is recorded as a difference the verdict counts.
  writeRecord(log, { suite, cwd: process.cwd(), args, differences: [{ part: 'shim', error: String(error.message) }] })
  process.stderr.write(`equivalence shim: ${error.message}\n`)
  process.exit(70)
}
const { candidateRun, differences } = observed
writeRecord(log, { suite, cwd: process.cwd(), args, differences })
process.stdout.write(candidateRun.stdout)
process.stderr.write(candidateRun.stderr)
process.exitCode = candidateRun.status
if (candidateRun.signal) process.kill(process.pid, candidateRun.signal)
