import { RuleTester } from 'eslint';
import tseslint from 'typescript-eslint';
import { importExtensionsRule } from '../../build/lint/imports.mjs';
import { synchronousFsRule } from '../../build/lint/synchronousFs.mjs';
const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });
tester.run('import-extensions', importExtensionsRule, {
	valid: ["import './module.js';", "import './style.css';", "import type { X } from 'electron';", "type X = import('./module.js').X;"],
	invalid: [
		"import './module';", "export * from './module.ts';", "export { x } from './module';", "const value = import('./module');", "type X = import('./module').X;",
	].map(code => ({ code, errors: [{ messageId: 'extension' }] })),
});
tester.run('no-sync-fs', synchronousFsRule, {
	valid: ["import { readFile } from 'node:fs'; readFile('file', () => {});", "import * as fs from 'node:fs'; function f(fs) { fs.readFileSync('file'); }"],
	invalid: [
		"import { readFileSync as read } from 'node:fs'; read('file');",
		"import * as disk from 'fs'; disk.readFileSync('file');",
		"import disk from 'node:fs'; disk['statSync']('file');",
	].map(code => ({ code, errors: [{ messageId: 'synchronous' }] })),
});
