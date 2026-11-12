import { httpTransport, type Transport } from '@muthuishere/klark-hub-ui';
import { notifyUnauthorized } from './api';
import { baseHref } from './base';

// klark's hub over HTTP + SSE. The base is read per call (tests and HUB_BASE_PATH set it on <base href>).
const t = () => httpTransport(baseHref(), { onStatus: (s) => { if (s === 401) notifyUnauthorized(); } });

export const transport: Transport = {
  machines: () => t().machines(),
  tree: (m) => t().tree(m),
  getChat: (m, a) => t().getChat(m, a),
  events: (m, a, o, h) => t().events(m, a, o, h),
  send: (m, a, text) => t().send(m, a, text),
  input: (m, a, text) => t().input(m, a, text),
  keys: (m, a, keys) => t().keys(m, a, keys),
  upload: (m, a, file) => t().upload(m, a, file),
  fileUrl: (m, a, path, line) => t().fileUrl(m, a, path, line),
};
