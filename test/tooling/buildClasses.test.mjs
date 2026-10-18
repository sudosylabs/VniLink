import { RuleTester } from 'eslint';
import tseslint from 'typescript-eslint';
import { buildClassesRule } from '../../build/lint/buildClasses.ts';

const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

const allow = [{ allow: ['build/lib/processes.ts'] }];

tester.run('vnilink/build-classes', buildClassesRule, {
	valid: [
		{ code: 'export async function buildRelease(root, signal) { }', filename: '/repo/build/lib/release.ts', options: allow },
		{ code: 'export const bundle = define("bundle", async () => { });', filename: '/repo/build/lib/bundles.ts', options: allow },
		{ code: 'export class ProcessRunner { }', filename: '/repo/build/lib/processes.ts', options: allow },
	],
	invalid: [
		{ code: 'export class ReleaseBuild { async run() { } }', filename: '/repo/build/lib/release.ts', options: allow, errors: [{ messageId: 'class' }] },
		{ code: 'const Task = class { };', filename: '/repo/build/lib/task.ts', options: allow, errors: [{ messageId: 'class' }] },
	],
});
