import { tsxImportNormalize } from './tsx-import-normalize'

const castValue = <number>(1 as unknown)

/** Returns `value` normalized by the shared helper; this file also holds a legacy cast. */
export function tsxImportWrapTwo(value: number) {
  return tsxImportNormalize(value)
}
