// Highlighting off the main thread for the file viewer (ADR 0011, G). Loads the
// highlight.js core and only the language this file needs, then returns one HTML
// string per line (spans closed at each line end and reopened on the next), so the
// page can virtualize lines. hljs escapes the source.
const base = "/lib/hljs-11.11.1/";
const COMMON = ["bash", "c", "cpp", "csharp", "css", "go", "java", "javascript", "json", "markdown", "python", "rust", "sql", "typescript", "xml", "yaml"];
let core = null;

async function hl(lang) {
  core ??= (await import(base + "core.min.js")).default;
  const want = lang ? [lang] : COMMON;
  for (const l of want) {
    if (!/^[a-z0-9-]+$/.test(l) || core.getLanguage(l)) continue;
    try { core.registerLanguage(l, (await import(base + "languages/" + l + ".min.js")).default); } catch { }
  }
  return core;
}

export function splitLines(html) {
  const out = [];
  const stack = [];
  let cur = "";
  const re = /(<span[^>]*>)|(<\/span>)|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    if (m[1]) { stack.push(m[1]); cur += m[1]; }
    else if (m[2]) { stack.pop(); cur += m[2]; }
    else {
      const parts = m[3].split("\n");
      for (let i = 0; i < parts.length; i++) {
        if (i > 0) {
          out.push(cur + "</span>".repeat(stack.length));
          cur = stack.join("");
        }
        cur += parts[i];
      }
    }
  }
  out.push(cur + "</span>".repeat(stack.length));
  return out;
}

self.onmessage = async (e) => {
  const { text, lang } = e.data;
  try {
    const h = await hl(lang);
    const r = lang && h.getLanguage(lang)
      ? h.highlight(text, { language: lang, ignoreIllegals: true })
      : h.highlightAuto(text.slice(0, 200000));
    // highlightAuto on a prefix only guesses the language; the whole file is then highlighted with it.
    const html = !lang && r.language ? h.highlight(text, { language: r.language, ignoreIllegals: true }).value : r.value;
    self.postMessage({ lines: splitLines(html), language: lang || r.language || "" });
  } catch (err) {
    self.postMessage({ error: String(err) });
  }
};
