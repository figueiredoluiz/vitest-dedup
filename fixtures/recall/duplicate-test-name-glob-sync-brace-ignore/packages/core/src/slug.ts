export function slug(input: string): string {
  return input.trim().toLowerCase().replace(/\s+/g, '-')
}
