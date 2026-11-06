// The module both benchmark files measure.
/** Splits `key=value` lines into pairs. */
export function parse(source: string): string[][] {
  return source.split('\n').map((line) => line.split('='))
}
