// GitHub Action runner for VitestDedup.
//
// Node 24, ESM, zero dependencies: this script runs via `runs: using: node24` in action.yml, which
// invokes it directly with the platform Node binary and no `npm install` step. There is no
// `node_modules`, so nothing here may `import` a package that is not part of Node itself.
//
// Pure logic (platform resolution, argument construction, output extraction, exit-code mapping) is
// factored into exported functions with no I/O, so index.test.mjs can drive them directly without a
// network, a filesystem, or a real vitest-dedup binary. Everything else in this file is the thin I/O
// shell around those functions: reading inputs, downloading and verifying the release, running it,
// and publishing results.
import {
  appendFileSync,
  chmodSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { createHash, randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { TARGETS, targetFor } from "../../../npm/lib/targets.mjs";

const REPO = "figueiredoluiz/vitest-dedup";

// ---------------------------------------------------------------------------
// Platform resolution
// ---------------------------------------------------------------------------

/**
 * Linux is the only platform where the same (platform, arch) pair maps to two different release
 * targets (glibc vs musl), so it is the only one that needs libc detection at all.
 *
 * `process.report` exposes `header.glibcVersionRuntime` only when the running Node binary itself
 * is linked against glibc; a musl-linked Node (e.g. Alpine) omits that field entirely. That makes
 * presence/absence of the field a reliable proxy for the host's libc, without shelling out to
 * `ldd` or reading `/etc/os-release`. Any failure to even ask the question (a report API that
 * throws) is treated as musl: musl binaries only fail loudly on glibc hosts (missing dynamic
 * linker), while a glibc binary run under musl fails silently/confusingly, so the safer guess
 * under uncertainty is the one that fails loud.
 */
export function detectLibc(report = process.report) {
  try {
    return report?.getReport()?.header?.glibcVersionRuntime ? "gnu" : "musl";
  } catch {
    return "musl";
  }
}

/**
 * Resolves the running host to a release target from the shared platform manifest. Returns
 * `undefined` rather than throwing so the caller can report the unsupported platform by name
 * instead of the download step failing with an opaque 404.
 */
export function releaseTarget(
  platform = process.platform,
  arch = process.arch,
  libc = platform === "linux" ? detectLibc() : undefined,
  targets = TARGETS,
) {
  return targetFor(platform, arch, libc, targets);
}

// ---------------------------------------------------------------------------
// Argument construction
// ---------------------------------------------------------------------------

/**
 * Split comma/newline input into trimmed, nonempty entries.
 */
function splitList(value = "") {
  return value
    .split(/[\n,]+/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

/**
 * Normalizes the `reporters` input into the list of reporters the caller actually asked for
 * (before the JSON reporter is force-added by `buildArguments`). Shared between `buildArguments`
 * and the output-publishing logic in `runAction` so the two can never disagree about, e.g.,
 * whether the html reporter was requested.
 */
export function resolveReporters(reportersInput) {
  const reporters = splitList(reportersInput || "terminal");
  return reporters.length === 0 ? ["terminal"] : reporters;
}

/**
 * Builds the vitest-dedup CLI argument vector from the action's inputs.
 *
 * The JSON reporter is force-added rather than substituted: the action's outputs are parsed from
 * the JSON report, so it must always run, but a user who asked for `terminal` still wants terminal
 * output in the log too. Silently replacing their choice would make the log go quiet with no
 * indication why.
 */
export function buildArguments(inputs) {
  if (
    inputs.failOnIncomplete !== undefined &&
    inputs.failOnIncomplete !== "" &&
    !["false", "true", "any", "reads", "limits"].includes(inputs.failOnIncomplete)
  ) {
    throw new Error(
      `invalid fail-on-incomplete input "${inputs.failOnIncomplete}"; expected false, true, any, reads, or limits`,
    );
  }

  const args = [inputs.path || "."];

  if (inputs.config) {
    args.push("--config", inputs.config);
  }
  if (inputs.threshold !== undefined && inputs.threshold !== "") {
    args.push("--threshold", inputs.threshold);
  }

  const reporters = resolveReporters(inputs.reporters);
  if (!reporters.includes("json")) {
    reporters.push("json");
  }
  args.push("--reporters", reporters.join(","));

  if (inputs.output) {
    args.push("--output", inputs.output);
  }

  for (const rule of splitList(inputs.rule)) {
    args.push("--rule", rule);
  }

  // Boolean CLI flags take no value, so the only sound test is exact string equality against
  // "true" -- GitHub Actions always passes boolean-looking inputs through as strings, and anything
  // else (empty, "false", "1", a typo) must not silently turn the flag on.
  if (inputs.noMetrics === "true") {
    args.push("--no-metrics");
  }
  if (inputs.globals === "true") {
    args.push("--globals");
  }
  // The CLI's selective modes read an `=`-value; "true" keeps the bare form for back-compat.
  if (inputs.failOnIncomplete === "true") {
    args.push("--fail-on-incomplete");
  } else if (["any", "reads", "limits"].includes(inputs.failOnIncomplete)) {
    args.push(`--fail-on-incomplete=${inputs.failOnIncomplete}`);
  }

  if (inputs.baseline) {
    args.push("--baseline", inputs.baseline);
  }
  // Only meaningful alongside a baseline, and the CLI rejects it without one. Passed through
  // rather than validated here so the CLI's own message is what the user sees.
  if (inputs.failOnNew !== undefined && inputs.failOnNew !== "") {
    args.push("--fail-on-new", inputs.failOnNew);
  }
  // `--changed-since` needs full history: `actions/checkout` defaults to depth 1, where the CLI
  // refuses outright rather than diffing against a truncated history.
  if (inputs.changedSince) {
    args.push("--changed-since", inputs.changedSince);
  }

  return args;
}

// ---------------------------------------------------------------------------
// Output extraction
// ---------------------------------------------------------------------------

/** Where a file-based reporter writes its report, given the `--output` prefix passed to the CLI. */
export function resolveReportPath(cwd, output, extension) {
  return resolve(cwd, `${output}.${extension}`);
}

/** Where the JSON reporter writes its report, given the `--output` prefix passed to the CLI. */
export function resolveJsonReportPath(cwd, output) {
  return resolveReportPath(cwd, output, "json");
}

/**
 * Converts a parsed JSON report plus the native exit code into the string-valued map that
 * action.yml's outputs declare. GitHub Actions outputs are always strings.
 *
 * When `report` is `null`/`undefined` -- the binary failed before writing one, or it was deleted
 * upfront and never replaced, see `runAction` -- this deliberately omits every finding-derived
 * output (`findings`, `errors`, `warnings`, `tests-analyzed`, `files-analyzed`, `incomplete`)
 * rather than reporting zeros. Zeros are indistinguishable from a genuinely clean run; omitting the
 * outputs entirely is the only way to avoid a stale or failed run being read as "no findings."
 */
/**
 * Checks that a parsed JSON report has the minimum shape `extractOutputs` needs to trust it:
 * `schemaVersion` present as a string, `findings` present as an array, and
 * `testsAnalyzed`/`filesAnalyzed` present as numbers.
 *
 * This exists because "no report file" is not the only way the binary can fail to hand this
 * action something useful. A parseable-but-empty object (`{}`), or a report truncated mid-write to
 * a value that still parses as valid JSON, sails straight past `existsSync`/`JSON.parse` and would
 * otherwise be read by `extractOutputs` as a genuinely clean, zero-finding run -- the same
 * indistinguishable-from-success failure the missing-report check exists to prevent, just one step
 * later. This is deliberately a shape check, not a deep validation: it is not this action's job to
 * fully re-validate the JSON report schema (that belongs to the reporter that produces it), only to
 * refuse to extract outputs from something that could not possibly be one.
 */
export function isValidReportShape(report) {
  return (
    typeof report === "object" &&
    report !== null &&
    typeof report.schemaVersion === "string" &&
    Array.isArray(report.findings) &&
    typeof report.testsAnalyzed === "number" &&
    typeof report.filesAnalyzed === "number"
  );
}

/**
 * Build string-valued Action outputs from the exit code and available report, returning only
 * the exit code when no report exists.
 */
export function extractOutputs(report, exitCode) {
  const outputs = { "exit-code": String(exitCode) };
  if (!report) {
    return outputs;
  }

  const findings = Array.isArray(report.findings) ? report.findings : [];
  let errors = 0;
  let warnings = 0;
  for (const finding of findings) {
    if (finding?.severity === "error") errors += 1;
    else if (finding?.severity === "warning") warnings += 1;
  }
  outputs.findings = String(findings.length);
  outputs.errors = String(errors);
  outputs.warnings = String(warnings);
  outputs["tests-analyzed"] = String(report.testsAnalyzed ?? 0);
  outputs["files-analyzed"] = String(report.filesAnalyzed ?? 0);
  outputs.incomplete = report.incomplete === true ? "true" : "false";
  // `findings`/`errors`/`warnings` above count every finding, baselined or not -- a baseline
  // changes what fails the gate, not what was found. These two say how that total splits, so a
  // workflow that reports on "new duplication" has the number without re-reading the report.
  const modes = report.modes ?? {};
  outputs.baselined = String(modes.baselined ?? 0);
  outputs["new-findings"] = String(modes.newFindings ?? 0);
  return outputs;
}

// ---------------------------------------------------------------------------
// Exit-code mapping
// ---------------------------------------------------------------------------

/**
 * Maps the native vitest-dedup exit code to the action's step outcome.
 *
 * The binary's contract is deliberately three-valued (0 clean, 1 quality gate failed, 2
 * operational error) so CI can distinguish "your tests are duplicated" from "the analyzer itself
 * broke." Collapsing either failure code into the other -- or into a generic "step failed" with no
 * distinguishing message -- would destroy that distinction right where it is most useful. So this
 * function never remaps the number: it only decides whether the step should fail, and attaches a
 * message that names which of the two failure modes happened. The actual native code is still
 * published untouched via the `exit-code` output and via `process.exitCode`.
 */
export function describeExitCode(exitCode) {
  if (exitCode === 0) {
    return { failed: false, message: null };
  }
  if (exitCode === 1) {
    return {
      failed: true,
      message: "VitestDedup quality gate failed: findings exceeded the configured threshold (exit code 1).",
    };
  }
  return {
    failed: true,
    message: `VitestDedup exited with an operational error, not a quality-gate failure (exit code ${exitCode}).`,
  };
}

// ---------------------------------------------------------------------------
// Checksum verification
// ---------------------------------------------------------------------------

/**
 * Parses a `sha256sum`-style checksum file (`<hex digest>  <filename>`) and returns the digest.
 * Split out from `verifyChecksum` so the parsing/validation logic is testable without touching
 * disk.
 */
export function expectedDigestFromChecksumFile(content) {
  const first = content.trim().split(/\s+/)[0] ?? "";
  if (!/^[0-9a-f]{64}$/i.test(first)) {
    throw new Error(`checksum file did not contain a sha256 digest: ${content.trim()}`);
  }
  return first.toLowerCase();
}

/**
 * Verifies the downloaded archive against its published digest before it is extracted and
 * *executed* in the user's CI.
 *
 * Be precise about what this does and does not buy, because an earlier version of this comment
 * overclaimed and a reviewer was right to call it out. The checksum file is produced by the same
 * release job as the archive and published to the same release, so it is NOT an independent trust
 * root. What it defends against:
 *
 *   - Truncated, corrupted, or partially-written downloads.
 *   - A single mutated artifact whose sidecar digest was not correspondingly updated.
 *
 * What it does NOT defend against: anyone who can publish release assets at all. A compromised
 * release workflow, a stolen token with release-write scope, or a malicious change to
 * `release.yml` can publish a backdoored archive *and* a matching digest, and this function will
 * report success. Transport integrity against a network attacker comes from TLS to github.com,
 * not from here.
 *
 * The independent trust root is the build-provenance attestation the release workflow generates
 * (`actions/attest-build-provenance`), which is signed by GitHub's OIDC identity rather than by
 * the artifact's own pipeline. Verifying it requires the `gh` CLI, which is present on hosted
 * runners but not guaranteed on self-hosted ones, so it is checked when available rather than
 * made a hard prerequisite -- see `verifyAttestation`.
 *
 * On mismatch the archive is deleted before throwing, so a bad download can never be picked up by
 * a later run reusing the same path, and the failure cannot be mistaken for a usable file.
 */
export function verifyChecksum(archivePath, checksumPath) {
  const expected = expectedDigestFromChecksumFile(readFileSync(checksumPath, "utf8"));
  const actual = createHash("sha256").update(readFileSync(archivePath)).digest("hex").toLowerCase();
  if (actual !== expected) {
    rmSync(archivePath, { force: true });
    throw new Error(
      `checksum mismatch for ${archivePath}: expected ${expected}, got ${actual}. The downloaded file has been deleted.`,
    );
  }
}

/** `gh`'s documented exit status for "authentication required"; any other failure stays fatal. */
const GH_EXIT_AUTH_REQUIRED = 4;

/** Releases built before tag-only publishing; their provenance records `refs/heads/main`. */
const RELEASES_WITHOUT_TAG_PROVENANCE = new Set(["0.1.0"]);

/**
 * Returns the `gh attestation verify` identity flags for `version`: the attestation must come from
 * this repository's release workflow, built from the version's tag. `--repo` alone would accept any
 * workflow or ref in the repository.
 */
export function attestationIdentity(repository, version) {
  const identity = ["--repo", repository, "--signer-workflow", `${repository}/.github/workflows/release.yml`];
  if (!RELEASES_WITHOUT_TAG_PROVENANCE.has(version)) {
    identity.push("--source-ref", `refs/tags/v${version}`);
  }
  return identity;
}

/**
 * Verifies the archive's build-provenance attestation, when the tooling to do so is present.
 *
 * This is the check `verifyChecksum` cannot be: the attestation is signed by GitHub's OIDC
 * identity, not by the release pipeline that produced the artifact, so it still means something if
 * that pipeline is compromised.
 *
 * Deliberately advisory rather than fatal when the verifier cannot run: `gh` missing (self-hosted
 * runners may not ship it) or unauthenticated (`gh` exits 4 without a token). A FAILED verification
 * is always fatal — only an *unavailable* verifier is tolerated, and it says so loudly, so the
 * difference is never silent.
 *
 * `gh` refuses every API call without credentials, and a JavaScript action's environment carries no
 * token unless one is passed in, so `token` (the `token` input, defaulting to `github.token`) is
 * given to the `gh` child as `GH_TOKEN` and to nothing else.
 *
 * @returns {"verified" | "unavailable" } — throws on an actual verification failure.
 */
export function verifyAttestation(archivePath, repository, version, runner = spawnSync, token = "") {
  const probe = runner("gh", ["--version"], { stdio: "ignore" });
  if (probe.error || probe.status !== 0) {
    warningAnnotation(
      `build-provenance attestation not verified: the \`gh\` CLI is unavailable on this runner. ` +
        `The SHA-256 digest was still checked, but it cannot attest to the artifact's origin.`,
    );
    return "unavailable";
  }
  const result = runner(
    "gh",
    ["attestation", "verify", archivePath, ...attestationIdentity(repository, version)],
    {
      stdio: "pipe",
      encoding: "utf8",
      env: token ? { ...withoutVariables(process.env, ["GH_TOKEN"]), GH_TOKEN: token } : process.env,
    },
  );
  // Advisory only when no token was supplied: with one, "authentication required" means the token
  // did not authenticate, which is a failure like any other.
  if (result.status === GH_EXIT_AUTH_REQUIRED && !token) {
    warningAnnotation(
      `build-provenance attestation not verified: the \`gh\` CLI is not authenticated. Leave the ` +
        `\`token\` input at its default (github.token) or pass a token. The SHA-256 digest was ` +
        `still checked, but it cannot attest to the artifact's origin.`,
    );
    return "unavailable";
  }
  if (result.status !== 0) {
    throw new Error(
      `build-provenance attestation failed for ${archivePath}. The archive's digest matched its ` +
        `published checksum, but its origin could not be verified against ${repository}. ` +
        `Refusing to execute it.\n${result.stderr ?? ""}`,
    );
  }
  return "verified";
}

// ---------------------------------------------------------------------------
// I/O shell
// ---------------------------------------------------------------------------

/**
 * Read an Action input from its exact or portable environment key, falling back only when both
 * are absent.
 */
function readInput(env, name, fallback) {
  const exact = `INPUT_${name.toUpperCase()}`;
  const portable = exact.replaceAll("-", "_");
  return env[exact] ?? env[portable] ?? fallback;
}

/**
 * Append a delimiter-framed GitHub output when configured, otherwise print the name/value
 * pair.
 */
function setOutput(env, name, value) {
  if (env.GITHUB_OUTPUT) {
    // The delimiter form (`name<<EOF\n...\nEOF`) is required because report paths and any future
    // multi-line output must round-trip safely; a plain `name=value` line breaks the moment the
    // value contains a newline.
    const delimiter = `vitest_dedup_${randomUUID()}`;
    appendFileSync(env.GITHUB_OUTPUT, `${name}<<${delimiter}\n${value}\n${delimiter}\n`);
  } else {
    console.log(`${name}=${value}`);
  }
}

/**
 * Encodes a workflow-command's *data* per GitHub's documented escaping so a message containing a
 * literal `%`, CR, or LF cannot be read as workflow-command syntax by the runner.
 *
 * This matters because both annotation helpers below carry untrusted text: an error message can
 * embed a path or identifier from the analyzed repository, and a warning can embed a diagnostic
 * derived from repository content. Without this encoding, a LF in that text would start what looks
 * like a second `::command` line in the log, letting analyzed-repository content forge additional
 * workflow commands. `%` must be encoded first -- encoding it after CR/LF would double-encode the
 * `%` those replacements just introduced.
 */
export function encodeAnnotationData(value) {
  return String(value).replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A");
}

/**
 * Emit a warning annotation through the supplied logger with workflow-control characters
 * escaped.
 */
export function warningAnnotation(message, log = console.log) {
  log(`::warning::${encodeAnnotationData(message)}`);
}

/**
 * Emit an escaped GitHub error annotation through the supplied logger.
 */
export function errorAnnotation(message, log = console.log) {
  log(`::error::${encodeAnnotationData(message)}`);
}

/**
 * Fetch a release asset into destination, rejecting unsuccessful or bodyless responses and
 * propagating stream failures.
 */
async function download(url, destination) {
  const response = await fetch(url, {
    redirect: "follow",
    headers: { "user-agent": "vitest-dedup-action" },
  });
  if (!response.ok || !response.body) {
    throw new Error(`failed to download ${url}: HTTP ${response.status}`);
  }
  await pipeline(response.body, createWriteStream(destination));
}

/**
 * Validates a (leading-`v`-stripped) version string against the exact grammar release.yml's
 * `resolve` job enforces on tag/dispatch input: `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$`.
 *
 * That workflow is the reference because it governs what can actually be published -- a version
 * this function accepts but the workflow would reject can never correspond to a real release, so
 * agreement is not optional. In particular this must reject a `+build` suffix and a hyphen inside
 * the prerelease identifier (e.g. `1.0.0+build` or `1.0.0-rc-1`): an earlier, looser pattern here
 * accepted both, let them pass validation, and then 404'd against every real release asset --
 * turning a validation bug into a confusing download failure instead of a clear error message.
 */
export function isValidReleaseVersion(version) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/.test(version);
}

/**
 * Downloads the release archive and its checksum for `version`/`target`, verifies the archive
 * before touching it any further, extracts it, and returns the executable binary's path with the
 * attestation verdict.
 */
async function installRelease(rawVersion, target, env) {
  const version = rawVersion.replace(/^v/, "");
  if (!isValidReleaseVersion(version)) {
    throw new Error(`invalid release version: ${rawVersion}`);
  }

  const installDirectory = join(
    env.RUNNER_TEMP || env.TMPDIR || tmpdir(),
    "vitest-dedup-action",
    version,
    target.key,
    randomUUID(),
  );
  mkdirSync(installDirectory, { recursive: true });

  // Matches the naming convention produced by .github/workflows/release.yml: the archive stem is
  // `vitest-dedup-<version>-<target key>` (the manifest key, e.g. "linux-x64-gnu" -- not the Rust
  // target triple), always packaged as .tar.gz regardless of platform.
  const assetStem = `vitest-dedup-${version}-${target.key}`;
  const archive = join(installDirectory, `${assetStem}.tar.gz`);
  const checksum = `${archive}.sha256`;
  const base = `https://github.com/${REPO}/releases/download/v${version}`;

  await download(`${base}/${assetStem}.tar.gz`, archive);
  await download(`${base}/${assetStem}.tar.gz.sha256`, checksum);

  // Non-negotiable: verify before the archive is extracted or executed. The digest proves
  // integrity; the attestation below proves origin. Neither alone is sufficient, and both run
  // before a single byte is executed.
  verifyChecksum(archive, checksum);
  const attestation = verifyAttestation(archive, REPO, version, spawnSync, readInput(env, "token", ""));

  const extracted = spawnSync("tar", ["-xf", archive, "-C", installDirectory], { stdio: "inherit" });
  if (extracted.error || extracted.status !== 0) {
    throw new Error(`failed to extract ${assetStem}.tar.gz`);
  }

  const binary = join(installDirectory, assetStem, target.binaryName);
  if (!existsSync(binary)) {
    throw new Error(`release archive did not contain the expected binary at ${binary}`);
  }
  if (target.platform !== "win32") {
    chmodSync(binary, 0o755);
  }
  return { binary, attestation };
}

/**
 * Credentials a JavaScript action's environment can carry: the `token` input (the runner exposes
 * inputs as `INPUT_*`), the variables `gh` authenticates with, and the runner's own service tokens.
 * The analyzer needs none of them.
 */
const ANALYZER_DENIED_VARIABLES = [
  "INPUT_TOKEN",
  "GH_TOKEN",
  "GITHUB_TOKEN",
  "GH_ENTERPRISE_TOKEN",
  "GITHUB_ENTERPRISE_TOKEN",
  "ACTIONS_RUNTIME_TOKEN",
  "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
];

/**
 * Returns a copy of `env` without the named variables. Names are matched case-insensitively because
 * Windows environment names are, and a copied `process.env` keeps whatever casing the variable was
 * set with (`Gh_Token` is still `GH_TOKEN` to a child).
 */
export function withoutVariables(env, names) {
  const denied = new Set(names.map((name) => name.toUpperCase()));
  return Object.fromEntries(Object.entries(env).filter(([name]) => !denied.has(name.toUpperCase())));
}

/** Returns the analyzer's environment: the given one without any credential variable. */
export function analyzerEnvironment(env) {
  return withoutVariables(env, ANALYZER_DENIED_VARIABLES);
}

/**
 * Read Action inputs, obtain and run the analyzer, validate its report and publish outputs;
 * return the effective exit code.
 */
export async function runAction({ env = process.env, cwd = process.cwd() } = {}) {
  const inputs = {
    path: readInput(env, "path", "."),
    config: readInput(env, "config", ""),
    threshold: readInput(env, "threshold", ""),
    reporters: readInput(env, "reporters", "terminal"),
    output: readInput(env, "output", "reports/vitest-dedup"),
    rule: readInput(env, "rule", ""),
    globals: readInput(env, "globals", "false"),
    noMetrics: readInput(env, "no-metrics", "false"),
    failOnIncomplete: readInput(env, "fail-on-incomplete", "false"),
    baseline: readInput(env, "baseline", ""),
    failOnNew: readInput(env, "fail-on-new", ""),
    changedSince: readInput(env, "changed-since", ""),
    version: readInput(env, "version", ""),
  };

  const outputPrefix = inputs.output || "reports/vitest-dedup";
  const jsonReportPath = resolveJsonReportPath(cwd, outputPrefix);
  const htmlRequested = resolveReporters(inputs.reporters).includes("html");
  // Computed unconditionally, even when this run did not request the html reporter: an earlier
  // invocation sharing the same output prefix may have left a `.html` behind, and a stale report
  // that a later artifact-upload step collects is indistinguishable from one this run produced.
  // `htmlRequested` still governs whether the `html-report` output is published.
  const htmlReportPath = resolveReportPath(cwd, outputPrefix, "html");

  // Delete any report left over from an earlier run in this workspace *before* anything else that
  // can fail -- including, critically, before `installRelease` (download, checksum verification,
  // attestation, platform resolution). Every one of those can throw, and if this cleanup ran only
  // after them (as it previously did), a failure in any one of them would leave an older report
  // sitting at this path for a later workflow step (e.g. an artifact upload) to pick up and
  // mistake for this run's output -- an operational failure silently presented as clean or
  // unrelated results.
  rmSync(jsonReportPath, { force: true });
  rmSync(htmlReportPath, { force: true });

  const args = buildArguments(inputs);

  // Escape hatch used by index.test.mjs (and available to anyone running the action against a
  // locally built binary): skip the network entirely when a binary path is provided directly.
  const { binary, attestation } = env.VITEST_DEDUP_BINARY
    ? { binary: resolve(env.VITEST_DEDUP_BINARY), attestation: null }
    : await installRelease(
        inputs.version,
        (() => {
          const target = releaseTarget();
          if (!target) {
            throw new Error(
              `unsupported platform: no VitestDedup release is published for ${process.platform}/${process.arch}`,
            );
          }
          return target;
        })(),
        env,
      );

  // `stdio: "inherit"` streams the child's stdout/stderr directly to the workflow log as it is
  // produced, rather than buffering it and printing it only after the process exits -- the
  // terminal reporter is meant to be watched live, not read after the fact.
  const result = spawnSync(binary, args, {
    cwd,
    stdio: "inherit",
    env: analyzerEnvironment(process.env),
  });

  let exitCode = result.status ?? 2;
  if (result.error) {
    errorAnnotation(`failed to start ${binary}: ${result.error.message}`);
    exitCode = 2;
  } else if (result.signal) {
    errorAnnotation(`vitest-dedup was terminated by signal ${result.signal}`);
    exitCode = 2;
  }

  // Only a report this invocation just produced may be read -- the stale copy was already removed
  // above, so its mere presence here proves this run wrote it.
  const reportProduced = existsSync(jsonReportPath);
  let report = reportProduced ? JSON.parse(readFileSync(jsonReportPath, "utf8")) : null;
  if (!reportProduced) {
    if (exitCode === 0) {
      // A native exit code of 0 is a promise that the run was clean *and fully reported*. No
      // report file breaks that promise -- the binary either crashed after already deciding on a
      // 0 status, or the report never landed at this path for some other reason -- and treating
      // that as a warning (as this branch previously did) left the step green with every
      // finding-related output silently omitted: a passing step with no findings, indistinguishable
      // from a genuinely clean analysis. This is this project's most frequently recurring failure
      // mode (see A1 in the PR review), so it is escalated to an operational failure instead:
      // override the exit code to 2 and fail the step loudly, rather than let a broken run masquerade
      // as a clean one.
      exitCode = 2;
      errorAnnotation(
        `vitest-dedup exited 0 but no report was found at ${jsonReportPath}. Treating this as an ` +
          "operational failure rather than a clean run: a native exit code of 0 promises a complete " +
          "report, and that promise was broken. Finding-related outputs (findings, errors, warnings, " +
          "tests-analyzed, files-analyzed, incomplete) will not be published.",
      );
    } else {
      warningAnnotation(
        `no report was found at ${jsonReportPath} after the run. Finding-related outputs (findings, ` +
          "errors, warnings, tests-analyzed, files-analyzed, incomplete) will not be published. This " +
          "does not mean the run was clean -- check the run's exit code and log output above for the " +
          "actual failure.",
      );
    }
  } else if (!isValidReportShape(report)) {
    // A1's sibling failure, and just as capable of defeating it: "the analyzer produced no report"
    // and "the analyzer produced a report this action cannot understand" are different operator
    // problems, but both must be operational failures, never a warning that lets zeroed-out
    // finding outputs pass as a clean run. A `{}`, or a report truncated mid-write to something
    // that still parses as valid JSON, would otherwise sail past the `existsSync` check above and
    // get read by `extractOutputs` as a genuine zero-finding run -- indistinguishable from success.
    // Escalate unconditionally, even if the native exit code already signaled a failure: this
    // action cannot trust anything it would extract from this file, so it must be treated the same
    // as no report at all for output purposes (see the `report = null` below), not merged with
    // whatever the native exit code otherwise meant.
    exitCode = 2;
    errorAnnotation(
      `the report at ${jsonReportPath} does not have the expected shape (schemaVersion: string, ` +
        "findings: array, testsAnalyzed/filesAnalyzed: number). Treating this as an operational " +
        "failure, not a clean run: the analyzer produced a report this action cannot understand. " +
        "A likely cause is that the pinned `version` input names a VitestDedup release whose report " +
        "schema this action's parsing code predates or postdates -- check that the action and the " +
        "requested version are compatible. Finding-related outputs will not be published.",
    );
    report = null;
  }

  const outputs = extractOutputs(report, exitCode);
  if (attestation) {
    outputs.attestation = attestation;
  }
  if (reportProduced) {
    outputs["json-report"] = jsonReportPath;
  }
  if (htmlRequested) {
    if (existsSync(htmlReportPath)) {
      outputs["html-report"] = htmlReportPath;
    } else {
      // Naming the exact expected path and the `output` input it was derived from, rather than
      // just saying "no html report", lets the operator tell at a glance whether the html reporter
      // simply didn't run or whether `--output` pointed somewhere they did not intend.
      warningAnnotation(
        `the html reporter was selected but no report was found at the expected path: ` +
          `${htmlReportPath} (derived from the "output" input: "${outputPrefix}"). If this path is ` +
          "not where you expected the html report, check the output input.",
      );
    }
  }

  // Outputs are published before the step is allowed to fail below, so a later step that reads
  // `outputs.errors` (e.g. to post a PR comment) still works even when this step's own exit code
  // is non-zero.
  for (const [name, value] of Object.entries(outputs)) {
    setOutput(env, name, value);
  }

  // `incomplete` is a core product guarantee, not decoration: when the analysis could not fully
  // cover the corpus, a clean report proves nothing, and that has to be impossible to miss in a CI
  // log even if nobody is looking at the `incomplete` output.
  if (outputs.incomplete === "true") {
    warningAnnotation(
      "VitestDedup could not fully cover the corpus (incomplete=true). The absence of a finding is not proof that none exists.",
    );
  }

  const { failed, message } = describeExitCode(exitCode);
  if (failed) {
    errorAnnotation(message);
  }

  return exitCode;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    // Preserve the native exit code (0/1/2) verbatim -- see describeExitCode's comment on why
    // remapping it would destroy the distinction the whole action exists to surface.
    process.exitCode = await runAction();
  } catch (error) {
    errorAnnotation(`vitest-dedup action: ${error.message}`);
    process.exitCode = 2;
  }
}
