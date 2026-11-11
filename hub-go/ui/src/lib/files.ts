const enc = encodeURIComponent;
/** In-app route of the file viewer. */
export const viewerHref = (machine: string, pane: string, path: string) => `/m/${enc(machine)}/f?path=${enc(path)}&pane=${enc(pane)}`;
export const filesApi = (machine: string, which: '' | '/markdown' | '/raw', path: string, pane: string, extra = '') =>
  `api/files/${enc(machine)}${which}?path=${enc(path)}&pane=${enc(pane)}${extra}`;
export const imageExt = /\.(png|jpe?g|gif|webp|svg|bmp)$/i;

export function sizeLabel(n?: number): string {
  if (n == null) return '';
  return n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`;
}
