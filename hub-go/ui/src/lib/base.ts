/** The prefix the hub is mounted at, read from <base href> (Go injects HUB_BASE_PATH). */
export function baseHref(doc: Document = document): string {
  const href = doc.querySelector('base')?.getAttribute('href') ?? '/';
  try {
    return new URL(href, doc.location?.href ?? 'http://localhost/').pathname;
  } catch {
    return '/';
  }
}

/** React Router basename: "/" at the root, "/klark" under a prefix (no trailing slash). */
export function routerBasename(doc: Document = document): string {
  const p = baseHref(doc);
  const t = p.replace(/\/+$/, '');
  return t === '' ? '/' : t;
}

/** A URL relative to the base. Never absolute: "api/machines" -> "/klark/api/machines". */
export function url(rel: string, doc: Document = document): string {
  const clean = rel.replace(/^\/+/, '');
  const b = baseHref(doc);
  return (b.endsWith('/') ? b : b + '/') + clean;
}

/** Strip the base from a same-origin href (for in-app navigation of links the server rendered). */
export function appPath(href: string, doc: Document = document): string | null {
  let u: URL;
  try {
    u = new URL(href, doc.location?.href ?? 'http://localhost/');
  } catch {
    return null;
  }
  if (u.origin !== (doc.location?.origin ?? u.origin)) return null;
  const b = routerBasename(doc);
  if (b === '/') return u.pathname + u.search + u.hash;
  if (u.pathname === b || u.pathname.startsWith(b + '/')) return u.pathname.slice(b.length) + u.search + u.hash || '/';
  return null;
}
