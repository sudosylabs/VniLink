// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import '../../../../media/workbench.css';
import { timeout } from '../../../../../base/common/async.js';
import { animationFrames, until } from '../../../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { runWithFakedTimers } from '../../../../../base/test/common/virtualScheduling/index.js';
import type { IDialogResult } from '../../../../services/dialogs/dialogService.js';
import type { FilesView } from '../../filesView.js';
import { click, crumbs, entry, FilesFixture, job, list, names, press } from './filesFixture.js';

const downloadsHere = { root: '0', path: 'Users/someone/Downloads' };

const downloadsThere = { root: '2', path: 'Users/someone/Downloads' };

suite('FilesView', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: FilesFixture;
	setup(() => {
		fixture = disposables.add(new FilesFixture());
	});
	test('the Files row opens a tab named for the computer', () => {
		const view = fixture.run();
		assert.strictEqual(view.id, 'files:studio');
		assert.strictEqual(view.label, 'Studio PC');
	});
	test('this computer reopens at its last folder, and the other opens at home', async () => {
		const { local, remote } = await fixture.open();
		assert.strictEqual(crumbs(local).join('/'), 'Users/someone/Downloads');
		assert.strictEqual(crumbs(remote).join('/'), 'C:/Users/someone');
	});
	test('folders come first, in natural order', async () => {
		const { local } = await fixture.open();
		assert.deepStrictEqual(names(local), ['Screenshots', 'invoice.pdf', 'notes.md', 'vnilink-demo.mov']);
	});
	test('the status bar counts what this computer\'s folder holds', async () => {
		const { view } = await fixture.open();
		assert.match(view.element.querySelector('.files-status-selection')?.textContent ?? '', /^This (Mac|PC|Computer): 4 items$/);
	});
	test('copy is not offered with nothing selected', async () => {
		const { view } = await fixture.open();
		assert.strictEqual(view.element.querySelector<HTMLButtonElement>('.files-copy-button')?.disabled, true);
	});
	test('the places menu lists the other computer\'s drives', async () => {
		const { remote } = await fixture.open();
		assert.deepStrictEqual(await fixture.places(remote), ['Home', '', 'C:', 'D:']);
	});
	test('a drive plugged in after Files opened shows next time, after the disk', async () => {
		const { local } = await fixture.open();
		fixture.localDisk.plug('0', 'Volumes/Backup');
		assert.deepStrictEqual(await fixture.places(local), ['Home', 'Downloads', '', 'Whole Disk', 'Backup']);
	});
	test('a drive opens from the places menu, and Back returns home', async () => {
		const { remote } = await fixture.open();
		await fixture.places(remote);
		fixture.runMenu('files.root.2');
		await until(() => crumbs(remote).join('/') === 'C:', 'The drive did not open');
		click(remote.querySelector('.files-icon-button[aria-label="Back"]'));
		await until(() => crumbs(remote).join('/') === 'C:/Users/someone', 'Back did not return home');
	});
	test('Enter opens the focused folder, and the other computer\'s folder is remembered', async () => {
		const { remote } = await fixture.open();
		await fixture.openRemoteDownloads(remote);
		assert.deepStrictEqual(fixture.state.get('files.lastFolder.studio.remote'), downloadsThere);
	});
	test('Shift extends a selection, and copy names what and where, in the pane marked active', async () => {
		const { view, local, remote } = await fixture.open();
		await fixture.openRemoteDownloads(remote);
		fixture.selectTwoHere(local);
		const copyButton = view.element.querySelector<HTMLButtonElement>('.files-copy-button');
		assert.ok(copyButton);
		await until(() => !copyButton.disabled, 'Selecting did not offer copy');
		assert.strictEqual(copyButton.getAttribute('aria-label'), 'Copy 2 to Downloads on Studio PC');
		assert.deepStrictEqual([local.classList.contains('active'), remote.classList.contains('active')], [true, false]);
	});
	suite('a copy', () => {
		let view: FilesView;
		let local: HTMLElement;
		let remote: HTMLElement;
		setup(async () => {
			({ view, local, remote } = await fixture.open());
			await fixture.openRemoteDownloads(remote);
			fixture.selectTwoHere(local);
			const copyButton = view.element.querySelector<HTMLButtonElement>('.files-copy-button');
			await until(() => copyButton?.disabled === false, 'Selecting did not offer copy');
			click(copyButton);
			await until(() => fixture.session.copies.length === 1, 'Copy did not start');
		});
		test('sends the selection to the other computer\'s open folder', () => {
			const [copy] = fixture.session.copies;
			assert.strictEqual(copy.from, 'local');
			assert.deepStrictEqual(copy.items.map(item => item.name), ['invoice.pdf', 'notes.md']);
			assert.deepStrictEqual({ root: copy.to.root, path: copy.to.path }, downloadsThere);
		});
		test('shows what is copied in place where it lands, with the transfer\'s progress, before it arrives', async () => {
			await until(() => remote.querySelectorAll('.files-row.arriving').length === 2, 'The copies did not show in the folder they land in');
			assert.deepStrictEqual(names(remote), ['Installers', 'invoice.pdf', 'notes.md', 'report-q3.xlsx']);
			const arriving = remote.querySelector('.files-row.arriving');
			assert.notStrictEqual(arriving?.querySelector('.files-ring'), null);
			assert.strictEqual(arriving?.querySelector('.files-cell-modified')?.textContent, '0 of 2 files');
		});
		test('is summed up in the status bar', () => {
			assert.ok(view.element.querySelector('.files-status-transfers')?.textContent.startsWith('Copying to Studio PC'));
		});
		test('a change the watch sees and a file landing, together, list the folder once', async () => {
			await until(() => remote.querySelectorAll('.files-row.arriving').length === 2, 'The copies did not show');
			const listed = fixture.remoteDisk.lists.length;
			const watched = performance.now();
			fixture.remoteDisk.watchers.get('2:Users/someone/Downloads')?.();
			fixture.session.set(fixture.session.jobs.map(item => ({ ...item, files_done: '1' })));
			await until(() => fixture.remoteDisk.lists.length > listed && remote.querySelector('.files-row.arriving .files-cell-modified')?.textContent === '1 of 2 files', 'The watch and a landing were not both seen');
			// The listing that follows comes a second after the first: only a computer that took that long may have it already.
			const lists = fixture.remoteDisk.lists.length - listed;
			assert.ok(lists === 1 || (lists === 2 && performance.now() - watched >= 1000), `The watch and a landing listed the folder ${String(lists)} times`);
		});
		test('what lands becomes ordinary rows without reloading, tinted for a moment', async () => {
			const listed = fixture.remoteDisk.lists.length;
			for (const item of fixture.session.copies[0].items) {
				fixture.remoteDisk.land(fixture.session.copies[0].to, entry(item.name, 'file', 1000));
			}
			fixture.session.set(fixture.session.jobs.map(item => ({ ...item, state: 'Complete', done_bytes: item.total_bytes, files_done: item.files_total })));
			await until(() => fixture.remoteDisk.lists.length > listed, 'The receiving pane did not list again when the copy completed');
			await until(() => remote.querySelectorAll('.files-row.arriving').length === 0 && remote.querySelectorAll('.files-row[data-tone="landed"]').length === 2, 'What arrived did not become ordinary rows, tinted for a moment');
		});
	});
	suite('a name already there', () => {
		let view: FilesView;
		let local: HTMLElement;
		setup(async () => {
			({ view, local } = await fixture.open());
			fixture.session.set([job('21', 'WaitingDecision', { direction: 'Download', label: 'notes.md', waiting_file: 'notes.md', files_total: '1', destination: { ...downloadsHere, items: [{ name: 'notes.md', folder: false }] } })]);
			await until(() => local.querySelector('.files-decision') !== null, 'The clash did not ask under the name');
		});
		test('asks under that name, for that file alone', () => {
			assert.strictEqual(local.querySelector('.files-decision-text')?.textContent, 'Studio PC is sending another “notes.md”');
			assert.strictEqual(local.querySelector('.files-decision-same')?.hasAttribute('hidden'), true);
		});
		test('does not show the file already there as being replaced', () => {
			const namesake = [...local.querySelectorAll('.files-row')].find(row => row.querySelector('.files-row-name')?.textContent === 'notes.md');
			assert.strictEqual(namesake?.querySelector('.files-ring'), null);
		});
		test('Replace answers it in place', async () => {
			click([...local.querySelectorAll<HTMLButtonElement>('.files-decision button')].find(button => button.textContent === 'Replace'));
			await until(() => fixture.session.resolutions.length === 1, 'Replace did not answer');
			assert.deepStrictEqual(fixture.session.resolutions, [['21', 'replace', false]]);
		});
		test('the status bar asks for the decision and goes to the question, where Enter answers through a dialog', async () => {
			const decisionButton = view.element.querySelector<HTMLButtonElement>('.files-status-decision');
			assert.strictEqual(decisionButton?.textContent, '1 needs your decision');
			click(decisionButton);
			await until(() => document.activeElement === list(local) && local.querySelector('.monaco-list-row.focused .files-decision') !== null, 'The status bar did not go to the question');
			fixture.answers.push({ button: 2, checked: false });
			press(list(local), 'Enter', 13);
			await until(() => fixture.session.resolutions.length === 1, 'Enter on the question did not ask');
			assert.strictEqual(fixture.session.resolutions[0][1], 'keepBoth');
		});
	});
	suite('the list of transfers', () => {
		let view: FilesView;
		let summary: HTMLButtonElement;
		setup(async () => {
			({ view } = await fixture.open());
			fixture.session.set([job('20', 'Interrupted', { files_total: '1', destination: { ...downloadsThere, items: [{ name: 'big.iso', folder: false }] } }), job('22', 'Complete', { label: 'Screenshots', done_bytes: '1000000' })]);
			const button = view.element.querySelector<HTMLButtonElement>('.files-status-transfers');
			assert.ok(button);
			summary = button;
		});
		test('the status bar says a transfer stopped', () => {
			assert.strictEqual(summary.textContent, '1 stopped');
		});
		test('opens from the status bar with every transfer, the finished ones too, grouped', () => {
			click(summary);
			const activity = view.element.querySelector<HTMLElement>('.files-activity');
			assert.ok(activity && !activity.hidden && document.activeElement === activity, 'The list of transfers did not open');
			assert.deepStrictEqual([...activity.querySelectorAll('.files-activity-heading')].map(node => node.textContent), ['Now', 'Earlier']);
			assert.deepStrictEqual([...activity.querySelectorAll('.files-activity-item button')].map(node => node.textContent), ['Resume']);
		});
		test('a stopped transfer keeps its progress ring, and a finished one shows it landed', () => {
			click(summary);
			const activity = view.element.querySelector<HTMLElement>('.files-activity');
			const marks = [...activity?.querySelectorAll<HTMLElement>('.files-activity-item') ?? []].map(item => `${item.dataset.state ?? ''}:${item.querySelector('.files-ring') ? 'ring' : item.querySelector<HTMLElement>('.files-mark')?.dataset.tone ?? 'none'}`);
			assert.deepStrictEqual(marks, ['Interrupted:ring', 'Complete:landed']);
		});
		test('Escape closes it and returns to the status bar', () => {
			click(summary);
			const activity = view.element.querySelector<HTMLElement>('.files-activity');
			assert.ok(activity);
			press(activity, 'Escape', 27);
			assert.strictEqual(view.element.querySelector<HTMLElement>('.files-activity')?.hidden, true);
			assert.strictEqual(document.activeElement, summary);
		});
	});
	suite('renaming', () => {
		let local: HTMLElement;
		let input: HTMLInputElement;
		setup(async () => {
			({ local } = await fixture.open());
			list(local).focus();
			press(list(local), 'Home', 36);
			press(list(local), 'ArrowDown', 40);
			press(list(local), 'F2', 113);
			await until(() => local.querySelector('.files-row-name input') !== null, 'Rename did not open in the row');
			const field = local.querySelector<HTMLInputElement>('.files-row-name input');
			assert.ok(field);
			input = field;
		});
		test('F2 renames in place, selecting the name without its extension', () => {
			assert.strictEqual(input.value, 'invoice.pdf');
			assert.strictEqual(input.selectionEnd, 'invoice'.length);
		});
		test('a taken name is refused and said, and the item kept', async () => {
			input.value = 'notes.md';
			input.dispatchEvent(new Event('input'));
			input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
			await until(() => !(local.querySelector<HTMLElement>('.files-notice')?.hidden ?? true), 'A taken name was not reported');
			assert.strictEqual(local.querySelector('.files-notice')?.textContent, 'That name is already taken here.');
			assert.ok(names(local).includes('invoice.pdf'));
		});
	});
	suite('deleting', () => {
		let local: HTMLElement;
		let remote: HTMLElement;
		setup(async () => {
			({ local, remote } = await fixture.open());
			list(local).focus();
			press(list(local), 'Home', 36);
			press(list(local), 'ArrowDown', 40);
		});
		test('deleting for good asks first, says it is permanent, and Cancel keeps everything', async () => {
			press(list(local), 'Delete', 46, true);
			await until(() => fixture.dialogs.length === 1, 'Deleting for good did not ask');
			assert.ok(fixture.dialogs[0].message.startsWith('Permanently delete “invoice.pdf” from'), fixture.dialogs[0].message);
			await timeout(0);
			assert.deepStrictEqual(fixture.localDisk.deletes, []);
			assert.ok(names(local).includes('invoice.pdf'));
		});
		test('asking to delete for good twice while the question is open asks once, and the answer deletes', async () => {
			let answer: (result: IDialogResult) => void = () => undefined;
			fixture.answers.push(new Promise<IDialogResult>(resolve => {
				answer = resolve;
			}));
			press(list(local), 'Delete', 46, true);
			press(list(local), 'Delete', 46, true);
			await until(() => fixture.dialogs.length === 1, 'Deleting for good did not ask');
			await animationFrames();
			assert.strictEqual(fixture.dialogs.length, 1);
			answer({ button: 0, checked: false });
			await until(() => fixture.localDisk.deletes.join() === 'invoice.pdf:gone', 'The confirmed delete did not run');
			await until(() => !names(local).includes('invoice.pdf'), 'The deleted item stayed listed');
		});
		test('Delete moves to the Trash at once, without asking', async () => {
			press(list(local), 'Delete', 46);
			await until(() => fixture.localDisk.deletes.at(-1) === 'invoice.pdf:trash', 'Delete did not move the item to the Trash');
			assert.strictEqual(fixture.dialogs.length, 0);
			await until(() => !names(local).includes('invoice.pdf'), 'The item moved to the Trash stayed listed');
		});
		test('where there is no Trash, Delete offers to delete for good, in the other computer\'s words', async () => {
			await fixture.openRemoteDownloads(remote);
			fixture.remoteDisk.noTrash.add('report-q3.xlsx');
			const sheet = names(remote).indexOf('report-q3.xlsx');
			list(remote).focus();
			press(list(remote), 'Home', 36);
			for (let step = 0; step < sheet; step++) {
				press(list(remote), 'ArrowDown', 40);
			}
			fixture.answers.push({ button: 0, checked: false });
			press(list(remote), 'Delete', 46);
			await until(() => fixture.remoteDisk.deletes.at(-1) === 'report-q3.xlsx:gone', 'A place with no Trash did not offer to delete for good');
			assert.strictEqual(fixture.dialogs.at(-1)?.message, '“report-q3.xlsx” can’t be moved to the Recycle Bin. Delete it permanently?');
		});
		test('a row\'s menu offers the Recycle Bin and deleting for good', async () => {
			remote.querySelector<HTMLElement>('.monaco-list-row')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, button: 2 }));
			await until(() => fixture.menu.some(action => action.id === 'files.delete'), 'The menu did not open on a row');
			assert.strictEqual(fixture.menu.find(action => action.id === 'files.delete')?.label, 'Delete');
			assert.ok(fixture.menu.some(action => action.id === 'files.deletePermanently'));
		});
	});
	test('Show Hidden Files, from the pane\'s menu, is remembered', async () => {
		const { local } = await fixture.open();
		click(local.querySelector('.files-icon-button[aria-label="More Actions"]'));
		fixture.runMenu('files.hidden');
		assert.strictEqual(fixture.state.get('files.showHidden'), true);
	});
	test('a file removed outside VniLink leaves this computer\'s list without a refresh', async () => {
		const { local } = await fixture.open();
		fixture.localDisk.removeOutside(downloadsHere, 'vnilink-demo.mov');
		await until(() => !names(local).includes('vnilink-demo.mov'), 'A file removed outside VniLink stayed listed here');
	});
	test('the other computer\'s open folder is watched, and a file removed there leaves the list', async () => {
		const { remote } = await fixture.open();
		await fixture.openRemoteDownloads(remote);
		assert.ok(fixture.remoteDisk.watchers.has('2:Users/someone/Downloads'));
		fixture.remoteDisk.removeOutside(downloadsThere, 'report-q3.xlsx');
		await until(() => !names(remote).includes('report-q3.xlsx'), 'A file removed on the other computer stayed listed');
	});
	suite('a long folder', () => {
		let remote: HTMLElement;
		setup(async () => {
			({ remote } = await fixture.open());
			await fixture.openRemoteDownloads(remote);
			list(remote).focus();
			press(list(remote), 'Home', 36);
			press(list(remote), 'Enter', 13);
			await until(() => crumbs(remote).at(-1) === 'Installers', 'Installers did not open');
		});
		test('the watch follows the open folder', () => {
			assert.ok(fixture.remoteDisk.watchers.has('2:Users/someone/Downloads/Installers'));
			assert.ok(!fixture.remoteDisk.watchers.has('2:Users/someone/Downloads'));
		});
		test('a path longer than its bar scrolls, keeping whole names and the open folder in view', async () => {
			// The bar is narrowed to the open folder's name and its chevron, measured here: names
			// and the buttons beside the bar are wider in some systems' fonts, so a fixed width, or
			// one worked out from the pane's, can leave no room for one whole name.
			const crumbArea = remote.querySelector<HTMLElement>('.files-crumb-area');
			const folderWidth = [...remote.querySelectorAll<HTMLElement>('.files-crumb')].at(-1)?.getBoundingClientRect().width;
			const pathWidth = remote.querySelector<HTMLElement>('.files-crumbs')?.scrollWidth;
			assert.ok(crumbArea && folderWidth !== undefined && pathWidth !== undefined && folderWidth + 24 < pathWidth, 'The open path is too short to scroll');
			crumbArea.style.flex = `0 0 ${String(folderWidth + 24)}px`;
			await animationFrames();
			const area = crumbArea.getBoundingClientRect();
			const steps = [...remote.querySelectorAll<HTMLElement>('.files-crumb')];
			const end = steps.at(-1)?.getBoundingClientRect();
			assert.ok(steps.every(step => step.scrollWidth <= step.clientWidth), 'A long path squeezed its folder names');
			assert.ok(end && end.right <= area.right + 1 && end.left >= area.left, `The open folder is not in view at the end of a long path: it spans ${String(end?.left)}–${String(end?.right)} in ${String(area.left)}–${String(area.right)}`);
			assert.ok((steps[0]?.getBoundingClientRect().left ?? 0) < area.left, 'A long path did not scroll');
		});
		test('End reaches the last row, which shows whole above the status bar', async () => {
			press(list(remote), 'End', 35);
			await until(() => names(remote).includes('setup-59.exe'), 'The end of a long folder is not reachable');
			await animationFrames(1);
			const last = [...remote.querySelectorAll<HTMLElement>('.monaco-list-row')].find(row => row.querySelector('.files-row-name')?.textContent === 'setup-59.exe');
			const box = remote.querySelector<HTMLElement>('.files-list')?.getBoundingClientRect();
			assert.ok(last && box);
			assert.ok(last.getBoundingClientRect().bottom <= box.bottom, `The last row is cut off: row ends at ${String(last.getBoundingClientRect().bottom)}, the list at ${String(box.bottom)}`);
		});
	});
	suite('the connection', () => {
		const notConnected = 'Not connected to Studio PC.';
		const title = (pane: HTMLElement): string => pane.querySelector('.files-state-title')?.textContent ?? '';
		test('a lost connection shows in the pane, which reconnects by itself once the computer can be reached', () => runWithFakedTimers({}, async () => {
			const { remote } = await fixture.open();
			fixture.session.end();
			await timeout(0);
			assert.strictEqual(title(remote), notConnected);
			fixture.session.ended = false;
			await timeout(1000);
			await until(() => remote.querySelector('.files-state:not([hidden])') === null && names(remote).length > 0, 'The pane did not reconnect by itself');
		}));
		test('Files turned off there is said, and this computer\'s pane keeps working', async () => {
			const { local, remote } = await fixture.open();
			fixture.session.end();
			await until(() => title(remote) === notConnected, 'A lost connection did not show in the pane');
			fixture.openings.push('turnedOff');
			click(remote.querySelector('.files-state-action'));
			await until(() => title(remote) === 'Studio PC doesn’t share its files.', 'Files turned off there is not said');
			assert.ok(names(local).length > 0);
		});
		test('closing the tab lets go of the connection', async () => {
			const { view } = await fixture.open();
			assert.strictEqual(await fixture.tabService.close(view.id), true);
			assert.ok(fixture.session.released >= 1);
		});
	});
});
