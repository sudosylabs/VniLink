import { RuleTester } from 'eslint';
import tseslint from 'typescript-eslint';
import { workbenchDomRule } from '../../build/lint/workbenchDom.mjs';

const tester = new RuleTester({ languageOptions: { parser: tseslint.parser } });

tester.run('vnilink/workbench-dom', workbenchDomRule, {
	valid: [
		{ code: "const row = append(parent, $('.device-row'));" },
		{ code: "button.addEventListener('click', run);" },
		{ code: 'this.actionBar.push(actions, { icon: true });' },
		{ code: 'element.click = undefined;' },
	],
	invalid: [
		{ code: "const row = document.createElement('div');", errors: [{ messageId: 'createElement' }] },
		{ code: 'this.pairing.importButton.click();', errors: [{ messageId: 'click' }] },
	],
});
