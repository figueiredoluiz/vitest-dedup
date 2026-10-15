import { tsxImportNormalize } from './tsx-import-normalize'

/** Returns `value` normalized by the shared helper. */
export function tsxImportWrapOne(value: number) {
  return tsxImportNormalize(value)
}
