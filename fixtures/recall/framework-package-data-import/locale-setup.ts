import ja from '@test-kit/locale/dist/ja.json'

/** Makes the locale table available to the calling file. */
export function installLocale() {
  globalThis.localeMessages = ja.messages
}
