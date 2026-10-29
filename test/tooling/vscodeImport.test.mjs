import assert from 'node:assert/strict';
import test from 'node:test';
import { flattenSource, localPath } from '../../build/vscodeImport.ts';

test('upstream paths map into src without the vs segment', () => {
	assert.equal(localPath('src/vs/base/browser/ui/list/listWidget.ts'), 'src/base/browser/ui/list/listWidget.ts');
	assert.throws(() => localPath('extensions/git/src/main.ts'));
});

test('copies keep upstream text apart from the declared license and import flattening', () => {
	const source = ` *  Licensed under the MIT License. See License.txt in the project root for license information.
// see vs/base/common/event.ts and 'vs/platform/log/common/log.js'
const keep = 'avs/base/x';`;
	assert.equal(flattenSource(source), ` *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
// see base/common/event.ts and 'platform/log/common/log.js'
const keep = 'avs/base/x';`);
});
