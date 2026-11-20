import { fileURLToPath } from 'node:url'

export default {
  resolve: { alias: { '~': fileURLToPath(new URL('./app', import.meta.url)) } },
}
