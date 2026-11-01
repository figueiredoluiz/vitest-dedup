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
