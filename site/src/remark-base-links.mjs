// Markdown links are written site-absolute ("/hub/run/"). Astro does not add
// `base` to them, so when the site is served under a sub-path (GitHub Pages:
// /<repo>/), prefix it here.
export function remarkBaseLinks(base) {
	const prefix = base.replace(/\/$/, '');
	return () => (tree) => {
		if (!prefix) return;
		const visit = (node) => {
			if (
				node.type === 'link' &&
				typeof node.url === 'string' &&
				node.url.startsWith('/') &&
				!node.url.startsWith('//') &&
				!node.url.startsWith(prefix + '/')
			) {
				node.url = prefix + node.url;
			}
			if (node.children) node.children.forEach(visit);
		};
		visit(tree);
	};
}
