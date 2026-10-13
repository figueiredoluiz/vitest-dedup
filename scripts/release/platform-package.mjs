// Derives the expected workspace manifest from `npm/prebuilt-targets.json`. The eight platform
// manifests are tracked so npm can lock local workspace links before a new version is published.
// The version gate compares every tracked manifest with this derived form, preventing drift.
//
// The mapping that needs care is `libc`. The manifest says `gnu`/`musl` because that is Rust's
// vocabulary and what the target triple uses; npm's `libc` field takes `glibc`/`musl`. Getting
// this wrong does not fail loudly -- it makes npm install the musl binary on a glibc host, where
// it happens to run, or refuse to install anything at all.
const NPM_LIBC = { gnu: 'glibc', musl: 'musl' }

const PLATFORM_LABEL = { darwin: 'macOS', linux: 'Linux', win32: 'Windows' }

/** Human-readable name used in the package description and README heading. */
export function platformDescription(target) {
  const platform = PLATFORM_LABEL[target.platform] ?? target.platform
  const libc = target.libc ? ` ${NPM_LIBC[target.libc] ?? target.libc}` : ''
  const abi = target.platform === 'win32' ? ' MSVC' : ''
  return `${platform} ${target.arch}${libc}${abi} native binary for vitest-dedup`
}

/**
 * Builds the full `package.json` for one platform package.
 *
 * `files` is an allowlist rather than an ignore list: a platform package must contain the binary
 * and its legal notices and nothing else, and an allowlist cannot accidentally ship a stray file
 * that lands in the directory during a release run.
 */
export function platformManifest(target, version) {
  const manifest = {
    name: target.packageName,
    version,
    description: platformDescription(target),
    os: [target.platform],
    cpu: [target.arch],
  }
  if (target.libc !== undefined) {
    const libc = NPM_LIBC[target.libc]
    if (!libc) {
      throw new Error(`no npm libc value for ${target.packageName}'s libc ${target.libc}`)
    }
    manifest.libc = [libc]
  }
  manifest.files = [`bin/${target.binaryName}`, 'README.md', 'LICENSE']
  manifest.repository = {
    type: 'git',
    url: 'git+https://github.com/figueiredoluiz/vitest-dedup.git',
  }
  manifest.homepage = 'https://github.com/figueiredoluiz/vitest-dedup'
  manifest.license = 'MIT'
  return manifest
}

/**
 * The README published inside a platform package.
 *
 * Its job is to answer the question someone lands on this page with: "why is this in my
 * node_modules and should I install it?" -- and to state the supply-chain properties, because a
 * package containing one prebuilt executable is exactly the shape people are right to be wary of.
 */
export function renderPlatformReadme({ name, description }) {
  return `# ${name}

${description}.

This is an optional platform package for
[vitest-dedup](https://www.npmjs.com/package/vitest-dedup), a static analyzer for duplicate and
near-duplicate Vitest tests.

**Do not install this package directly.** Install the main package, which selects the right
platform package for you:

\`\`\`bash
npm install --save-dev vitest-dedup
# or
npm install --global vitest-dedup
\`\`\`

This package contains only the native \`vitest-dedup\` executable for its target platform, plus
package metadata, the licence, and this README.

Supply-chain notes:

- no runtime dependencies;
- no install scripts, and nothing downloaded at install time;
- published from the project's GitHub Actions release workflow using npm trusted publishing, with
  provenance attached;
- verify with \`npm audit signatures\` from an installed project.
`
}
