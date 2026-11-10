// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import { remarkBaseLinks } from './src/remark-base-links.mjs';

// GitHub Pages serves the site at https://<owner>.github.io/<repo>/.
// The workflow sets SITE and BASE; locally both default to the root.
const site = process.env.SITE ?? 'http://localhost:4321';
const base = process.env.BASE ?? '/';

export default defineConfig({
	site,
	base,
	output: 'static',
	trailingSlash: 'always',
	markdown: { remarkPlugins: [remarkBaseLinks(base)] },
	integrations: [
		starlight({
			title: 'klark',
			// Open in the light Paper theme (the hub's default) unless the visitor chose one.
			head: [
				{
					tag: 'script',
					content:
						"try{if(localStorage.getItem('starlight-theme')===null)localStorage.setItem('starlight-theme','light')}catch(e){}",
				},
			],
			description:
				'A terminal workspace for coding agents, and a self-hosted web hub to watch and answer them.',
			// Fails the build on a broken internal link or anchor. Relative links are allowed
			// only for the landing hero buttons (frontmatter links get no base prefix).
			plugins: [starlightLinksValidator({ errorOnLocalLinks: false, errorOnRelativeLinks: false })],
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/muthuishere/klark' },
			],
			editLink: {
				baseUrl: 'https://github.com/muthuishere/klark/edit/master/site/',
			},
			customCss: ['./src/styles/custom.css'],
			lastUpdated: false,
			sidebar: [
				{
					label: 'Getting started',
					items: [
						{ slug: 'start/install' },
						{ slug: 'start/sessions' },
						{ slug: 'start/panes-and-tabs' },
					],
				},
				{
					label: 'Agents',
					items: [
						{ slug: 'agents/detection' },
						{ slug: 'agents/messaging' },
						{ slug: 'agents/skill' },
					],
				},
				{
					label: 'Hub',
					items: [
						{ slug: 'hub/run' },
						{ slug: 'hub/kubernetes' },
						{ slug: 'hub/sign-in' },
						{ slug: 'hub/ldap' },
						{ slug: 'hub/machines' },
						{ slug: 'hub/expose' },
					],
				},
				{
					label: 'The web app',
					items: [
						{ slug: 'app/tour' },
						{ slug: 'app/chat' },
						{ slug: 'app/files' },
						{ slug: 'app/idle' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ slug: 'reference/cli' },
						{ slug: 'reference/hub-config' },
						{ slug: 'reference/http-api' },
						{ slug: 'reference/link-protocol' },
						{ slug: 'reference/security' },
						{ slug: 'reference/performance' },
						{ slug: 'reference/design-records' },
					],
				},
				{
					label: 'Help',
					items: [{ slug: 'help/faq' }],
				},
			],
		}),
	],
});
