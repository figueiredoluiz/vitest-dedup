export function route(path: string): { name: string; id: string } | null {
  const match = /^\/users\/(\d+)$/.exec(path)
  return match ? { name: 'user', id: match[1] } : null
}
