// The entry point shared by the gates that are pure readers.
//
// A gate here is written as a `runCli(argv) -> { status, lines }` function so that every rejection
// it can reach is asserted by its own test rather than only by launching a subprocess. What is left
// over -- decide whether this module was executed or imported, call `runCli`, turn a thrown error
// into a failure instead of a stack trace, print to the right stream, exit with the status -- is
// identical in every one of them, and the duplication gate caught it the moment a second gate was
// written to the same shape.
//
// Two details are load-bearing rather than incidental:
//
//   - **A throw is a failure, never a pass.** A gate that dies partway through has not measured
//     what it claims to measure, so the `catch` exits 1 with the gate's name. Letting the exception
//     propagate would also exit non-zero today, but it prints a stack trace where a reader expects
//     a verdict, and nothing stops a future caller from catching it.
//   - **Failures go to stderr, successes to stdout.** `npm run check` chains gates with `&&` and
//     its output is read in a terminal; a failure that arrives on stdout is indistinguishable from
//     the success it replaced.

import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

/**
 * The file URLs a main module may be reported under: of `path` with symlinks resolved, which is what
 * Node reports by default (`/tmp` on macOS is `/private/tmp`), and as written, which is what
 * `--preserve-symlinks-main` reports. Matching only one would make a gate started through a link
 * look imported, and it would exit 0 without running.
 *
 * @param {string} path The script path from `argv[1]`.
 * @returns {string[]} The candidate file URLs.
 */
function mainModuleUrls(path) {
  const written = pathToFileURL(path).href;
  try {
    return [pathToFileURL(realpathSync(path)).href, written];
  } catch {
    return [written];
  }
}

/**
 * Runs `runCli` as a command-line gate when `moduleUrl` is the module Node was asked to execute.
 * Returns `false` without doing anything when the module was merely imported -- which is what makes
 * a gate's own test able to import it.
 *
 * `name` appears in the message a thrown error becomes, so it should read as the gate's subject
 * ("documentation accuracy", "attribution leak") rather than as a file name.
 */
export function runAsGate(
  moduleUrl,
  runCli,
  name,
  { argv = process.argv, exit = process.exit, log = console.log, error = console.error } = {},
) {
  if (!argv[1] || !mainModuleUrls(argv[1]).includes(moduleUrl)) return false;
  let result;
  try {
    result = runCli(argv.slice(2));
  } catch (cause) {
    error(`\nFAIL: ${name} gate could not complete a scan.\n  ${cause.message}`);
    exit(1);
    return true;
  }
  const sink = result.status === 0 ? log : error;
  for (const line of result.lines) sink(line);
  exit(result.status);
  return true;
}
