// Builds release/: a self-contained package root (what the hub-ui-v* tag holds and what npm publishes):
// dist/index.js + .d.ts, style.css, README.md, LICENSE and a package.json with nothing to run at install.
import { execFileSync } from 'node:child_process';
import { copyFileSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, 'release');
const require = createRequire(import.meta.url);
const bin = (pkg, file) => path.join(path.dirname(require.resolve(pkg + '/package.json')), file);
const run = (file, args) => execFileSync(process.execPath, [file, ...args], { cwd: here, stdio: 'inherit' });

rmSync(out, { recursive: true, force: true });
run(bin('vite', 'bin/vite.js'), ['build']);
run(bin('typescript', 'bin/tsc'), ['-p', 'tsconfig.build.json']);
copyFileSync(path.join(here, 'style.css'), path.join(out, 'style.css'));
copyFileSync(path.join(here, 'README.md'), path.join(out, 'README.md'));
copyFileSync(path.join(here, '../../../../LICENSE'), path.join(out, 'LICENSE'));
const pkg = JSON.parse(readFileSync(path.join(here, 'package.json'), 'utf8'));
delete pkg.scripts;
delete pkg.devDependencies;
writeFileSync(path.join(out, 'package.json'), JSON.stringify(pkg, null, 2) + '\n');
console.log('release/ ready');
