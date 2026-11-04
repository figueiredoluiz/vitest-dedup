import { fileURLToPath } from 'node:url';

export default {
  root: fileURLToPath(new URL('.', import.meta.url)),
  test: { sequence: { hooks: 'list' }, maxWorkers: 1 },
};
