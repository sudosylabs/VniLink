import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const bundle = await build({ stdin: { contents: `export { TabService } from './src/workbench/services/tabs/common/tabServiceImpl.js';`, loader: 'ts', resolveDir: root }, bundle: true, write: false, platform: 'node', format: 'esm', target: 'node22', tsconfig: `${root}tsconfig.json` });
const { TabService } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);
function tab(id, options = {}) {
	return { id, label: id, actions: [], dispose() { }, ...options };
}
test('tab ownership preserves inactive views, deduplicates inputs and disposes once', async () => {
	const tabs = new TabService(); let firstDisposed = 0; let duplicateDisposed = 0; let secondDisposed = 0;
	const first = tab('first', {
		dispose() {
			firstDisposed++;
		}
	});
	const second = tab('second', {
		dispose() {
			secondDisposed++;
		}
	});
	tabs.open(first); tabs.open(second); tabs.select('first'); assert.equal(secondDisposed, 0);
	tabs.open(tab('first', {
		dispose() {
			duplicateDisposed++;
		}
	})); assert.equal(duplicateDisposed, 1); assert.equal(tabs.tabs.length, 2);
	tabs.move('first', 1); assert.deepEqual(tabs.tabs.map(input => input.id), ['second', 'first']);
	assert.equal(await tabs.close('first'), true); assert.equal(tabs.activeTab, second); assert.equal(firstDisposed, 1);
	tabs.dispose(); tabs.dispose(); assert.equal(firstDisposed, 1); assert.equal(secondDisposed, 1);
});
test('actions capture their original view and abort when that view closes', async () => {
	const tabs = new TabService(); let finish; let signal; const completed = [];
	const action = {
		id: 'run', label: 'Run', async run(input) {
			signal = input; await new Promise(resolve => {
				finish = resolve;
			}); completed.push('first');
		}
	};
	const first = tab('first', { actions: [action] }); const second = tab('second', {
		actions: [{
			id: 'run', label: 'Run', run() {
				completed.push('second');
			}
		}]
	});
	tabs.open(first); const running = tabs.executeAction('run'); assert.equal(tabs.isActionEnabled(first, action), false);
	tabs.open(second); await tabs.executeAction('run'); assert.deepEqual(completed, ['second']);
	await tabs.close('first'); assert.equal(signal.aborted, true); finish(); await running;
	assert.equal(tabs.activeTab, second); assert.deepEqual(completed, ['second', 'first']); tabs.dispose();
});
test('concurrent close requests share approval and service disposal fences late approval', async () => {
	const tabs = new TabService(); let approve; let approvals = 0; let disposed = 0;
	const input = tab('guarded', {
		canClose() {
			approvals++; return new Promise(resolve => {
				approve = resolve;
			});
		}, dispose() {
			disposed++;
		}
	});
	tabs.open(input); const first = tabs.close(input.id); const second = tabs.close(input.id); assert.equal(first, second);
	await Promise.resolve(); assert.equal(approvals, 1); tabs.dispose(); approve(true);
	assert.equal(await first, false); assert.equal(disposed, 1); assert.equal(tabs.tabs.length, 0);
});
