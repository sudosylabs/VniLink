import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('CI installs from the prerequisites manifest, reports the doctor first and pins Node with .nvmrc', async () => {
	const ci = await readFile(join(root, '.github/workflows/ci.yml'), 'utf8');
	assert.match(ci, /prerequisites\.ts packages debian /);
	assert.match(ci, /prerequisites\.ts packages fedora /);
	assert.match(ci, /prerequisites\.ts packages msys2 /);
	assert.equal((ci.match(/npm run doctor -- --require/g) ?? []).length, 5, 'every CI job but Windows (whose script runs it) reports the doctor first');
	assert.match(await readFile(join(root, 'scripts/test-windows.ps1'), 'utf8'), /'doctor', '--', '--require'/);
	assert.doesNotMatch(ci, /node-version:/, 'CI reads the Node version from .nvmrc');
	assert.match(ci, /node-version-file: \.nvmrc/);
	for (const job of ['macos:', 'ubuntu:', 'fedora:', 'windows:', 'linux-container:']) {
		assert.match(ci, new RegExp(`^  ${job}$`, 'm'), `CI has no ${job} job`);
	}
	const scripts = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).scripts;
	assert.match(scripts['test:windows'], /scripts\/test-windows\.ps1/);
	assert.equal(scripts.precommit, undefined);
	assert.match(scripts.validate, /npm run check && cargo fmt --all -- --check && npm run test:rust && npm test/);
});

test('the pre-commit hook checks staged files only; the full suite is opt-in before a push', async () => {
	const preCommit = await readFile(join(root, '.husky/pre-commit'), 'utf8');
	assert.doesNotMatch(preCommit, /npm (test|run (check|validate|precommit)\b(?!:))/);
	assert.match(preCommit, /npm run check:hygiene/);
	assert.match(preCommit, /lint-staged/);
	const { default: stages } = await import('../../.lintstagedrc.mjs');
	const typescript = stages['{src,test,build}/**/*.ts'];
	assert.equal(typescript([join(root, 'src/workbench/parts/titlebar/titlebarPart.ts')]).at(-1), 'npm run typecheck');
	assert.equal(stages['crates/**/*.rs'](['crates/vnilink/src/lib.rs']), 'cargo fmt --all -- --check');
	assert.match(await readFile(join(root, '.husky/pre-push'), 'utf8'), /vnilink\.prePush\)" = "full" \][\s\S]*npm run validate/);
});
