// Re-exports a missing module's `then`: reading it is a fixture read, not the runtime awaiting.
export { then as enabled } from './missing-module'
