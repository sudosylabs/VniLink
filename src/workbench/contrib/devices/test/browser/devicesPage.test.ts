// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { timeout } from '../../../../../base/common/async.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import { CreateInvitationAction, ImportInvitationAction } from '../../deviceActions.js';
import { DevicePageFixture, LAB, OFFICE, screenshot, STUDIO, TestScreenSnapshotService } from './devicePageFixture.js';

/** Lets the page finish what a promise it awaited started: every promise callback runs before the next task. */
function settled(): Promise<void> {
	return timeout(0);
}

function pressKey(target: Element, key: string, keyCode: number): void {
	target.dispatchEvent(new KeyboardEvent('keydown', { key, keyCode, bubbles: true }));
}

/** The list row that holds `row`; the list keeps focus and selection on these. */
function listRow(row: Element): HTMLElement {
	const element = row.closest<HTMLElement>('.monaco-list-row');
	assert.ok(element, 'the device row is inside a list row');
	return element;
}

/** Clicks `element` as a pointer does. */
function click(element: Element | null | undefined): void {
	assert.ok(element, 'the element to click exists');
	element.dispatchEvent(new MouseEvent('click', { bubbles: true }));
}

function shown(element: Element | null): boolean {
	return element instanceof HTMLElement && element.checkVisibility();
}

suite('DevicesPage', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: DevicePageFixture;
	let page: HTMLElement;
	const panel = (): HTMLElement | null => page.querySelector<HTMLElement>('.device-panel');
	setup(() => {
		fixture = disposables.add(new DevicePageFixture());
		page = fixture.page.element;
	});
	suite('list', () => {
		test('renders one row per device', () => {
			assert.deepStrictEqual([0, 1, 2].map(index => fixture.row(index).textContent.includes(fixture.devices[index].name)), [true, true, true]);
		});
		test('presents no device toolbar', () => {
			assert.strictEqual(page.querySelector('.devices-toolbar'), null);
		});
		test('devices are an accessible list of list items', () => {
			assert.strictEqual(page.querySelector('.device-list .monaco-list')?.getAttribute('role'), 'list');
			assert.strictEqual(listRow(fixture.row(0)).getAttribute('role'), 'listitem');
		});
		test('selecting a device opens its details', () => {
			click(fixture.row(1));
			assert.strictEqual(panel()?.hidden, false);
		});
		test('selecting a device dials it', () => {
			click(fixture.row(1));
			assert.deepStrictEqual(fixture.connections, [LAB]);
		});
		test('selecting a device opens no tab', () => {
			const tabs = document.querySelectorAll('[role="tab"]').length;
			click(fixture.row(1));
			assert.strictEqual(document.querySelectorAll('[role="tab"]').length, tabs);
		});
		test('a presence update keeps the selection', () => {
			click(fixture.row(1));
			fixture.setPresence(LAB, 'connecting');
			assert.strictEqual(listRow(fixture.row(1)).getAttribute('aria-selected'), 'true');
		});
		test('a presence update is rendered on its row', () => {
			click(fixture.row(1));
			fixture.setPresence(LAB, 'connecting');
			assert.ok(fixture.row(1).textContent.includes('Connecting'));
		});
		test('arrow keys move focus along the list', () => {
			click(fixture.row(1));
			const list = page.querySelector<HTMLElement>('.device-list .monaco-list');
			assert.ok(list);
			list.focus();
			pressKey(list, 'ArrowDown', 40);
			assert.ok(listRow(fixture.row(2)).classList.contains('focused'));
		});
		test('closing the details returns focus to the selected row', () => {
			click(fixture.row(1));
			const list = page.querySelector<HTMLElement>('.device-list .monaco-list');
			click(page.querySelector<HTMLButtonElement>('.device-close'));
			assert.strictEqual(document.activeElement, list);
			assert.ok(listRow(fixture.row(1)).classList.contains('focused'));
			assert.strictEqual(panel()?.hidden, true);
		});
	});
	suite('services', () => {
		const services = (): Record<string, string> => Object.fromEntries([...page.querySelectorAll('.device-service')].map(row => [row.querySelector('.device-service-label')?.textContent ?? '', row.querySelector('.device-service-status')?.textContent ?? '']));
		const serviceButton = (name: string): HTMLButtonElement | undefined => [...page.querySelectorAll<HTMLButtonElement>('button.device-service')].find(button => button.textContent.startsWith(name));
		const cancel = (): HTMLButtonElement | null => page.querySelector<HTMLButtonElement>('.device-service-cancel:not([hidden])');
		setup(async () => {
			fixture.report(STUDIO, [
				{ capability: 'desktopControl', status: 'available' }, { capability: 'desktopView', status: 'available' },
				{ capability: 'files', status: 'turnedOff' }, { capability: 'terminal', status: 'needsPermission' },
				{ capability: 'camera', status: 'unavailable' }, { capability: 'systemRestart', status: 'unsupported' }, { capability: 'systemShutdown', status: 'available' },
			]);
			click(fixture.row(0));
			await settled();
		});
		test('each service row says what the computer reports', () => {
			assert.deepStrictEqual(services(), {
				'Remote desktop': 'Available', 'View screen only': 'Available', 'Terminal': 'Needs permission on that computer', 'Files': 'Turned off by its owner',
				'Camera': 'Unavailable right now', 'Lock screen': 'Not supported there', 'Restart or shut down': 'Available',
			});
		});
		test('the services are a list in which the seven openable services are interactive', () => {
			assert.strictEqual(page.querySelector('.device-services')?.getAttribute('role'), 'list');
			assert.strictEqual(page.querySelectorAll('button.device-service, .device-service[tabindex]').length, 7);
		});
		test('lock and restart follow what the computer allows', () => {
			assert.strictEqual(serviceButton('Lock screen')?.disabled, true);
			assert.strictEqual(serviceButton('Restart or shut down')?.disabled, false);
		});
		test('camera is a disabled button while the computer cannot open it', () => {
			assert.strictEqual(serviceButton('Camera')?.disabled, true);
		});
		test('files is a disabled button while its owner has turned it off', () => {
			assert.strictEqual(serviceButton('Files')?.disabled, true);
		});
		test('terminal is a disabled button in its list item while it needs permission', () => {
			const terminal = serviceButton('Terminal');
			assert.strictEqual(terminal?.disabled, true);
			assert.strictEqual(terminal.parentElement?.tagName, 'LI');
		});
		test('an unavailable service is marked disabled', () => {
			assert.strictEqual(page.querySelector('.device-service[data-status="turnedOff"]')?.getAttribute('aria-disabled'), 'true');
		});
		test('an availability change refreshes the rows', async () => {
			fixture.report(STUDIO, [{ capability: 'files', status: 'available' }, { capability: 'terminal', status: 'available' }]);
			await settled();
			assert.strictEqual(services().Files, 'Available');
			assert.strictEqual(services()['Remote desktop'], 'Not supported there');
		});
		test('an available terminal opens the terminal on the selected computer', async () => {
			fixture.report(STUDIO, [{ capability: 'files', status: 'available' }, { capability: 'terminal', status: 'available' }]);
			await settled();
			const terminal = serviceButton('Terminal');
			assert.strictEqual(terminal?.disabled, false);
			assert.strictEqual(terminal.getAttribute('aria-disabled'), 'false');
			click(terminal);
			assert.deepStrictEqual(fixture.commands, [['vnilink.terminal.open', STUDIO]]);
		});
		test('an available camera opens on the selected computer', async () => {
			fixture.report(STUDIO, [{ capability: 'camera', status: 'available' }, { capability: 'systemShutdown', status: 'available' }]);
			await settled();
			click(serviceButton('Camera'));
			assert.deepStrictEqual(fixture.commands.at(-1), ['vnilink.camera.open', STUDIO]);
		});
		test('restart or shut down asks through its command for the selected computer', () => {
			click(serviceButton('Restart or shut down'));
			assert.deepStrictEqual(fixture.commands.at(-1), ['vnilink.system.restartOrShutDown', STUDIO]);
		});
		test('no Cancel shows with nothing to cancel', () => {
			assert.strictEqual(cancel(), null);
		});
		test('a restart countdown shows on its own row, disabled, with Cancel', () => {
			fixture.systemActions.set(STUDIO, { action: 'restart', phase: 'scheduled', message: 'Restarting in 24 s' });
			assert.strictEqual(services()['Restart or shut down'], 'Restarting in 24 s');
			assert.strictEqual(serviceButton('Restart or shut down')?.disabled, true);
			assert.strictEqual(services()['Lock screen'], 'Not supported there');
			assert.strictEqual(cancel()?.textContent, 'Cancel');
		});
		test('Cancel cancels on the selected computer', () => {
			fixture.systemActions.set(STUDIO, { action: 'restart', phase: 'scheduled', message: 'Restarting in 24 s' });
			click(cancel());
			assert.deepStrictEqual(fixture.commands.at(-1), ['vnilink.system.cancel', STUDIO]);
		});
		test('the outcome replaces the countdown, and the row then returns to its availability', () => {
			fixture.systemActions.set(STUDIO, { action: 'restart', phase: 'scheduled', message: 'Restarting in 24 s' });
			fixture.systemActions.set(STUDIO, { action: 'restart', phase: 'ended', message: 'Cancelled there' });
			assert.strictEqual(services()['Restart or shut down'], 'Cancelled there');
			assert.strictEqual(cancel(), null);
			assert.strictEqual(serviceButton('Restart or shut down')?.disabled, false);
			fixture.systemActions.set(STUDIO, undefined);
			assert.strictEqual(services()['Restart or shut down'], 'Available');
		});
		test('an offline computer reports every service offline', async () => {
			click(fixture.row(1));
			await settled();
			assert.deepStrictEqual(new Set(Object.values(services())), new Set(['Offline']));
		});
		test('a reconnecting computer reports every service reconnecting and disabled', async () => {
			click(fixture.row(2));
			await settled();
			assert.deepStrictEqual(new Set(Object.values(services())), new Set(['Reconnecting']));
			assert.ok([...page.querySelectorAll('.device-service')].every(row => row.getAttribute('aria-disabled') === 'true'));
		});
	});
	suite('details', () => {
		setup(async () => {
			click(fixture.row(0));
			await settled();
		});
		test('show the device id, computer, system and addresses', () => {
			const facts = Object.fromEntries([...page.querySelectorAll('.device-facts > div')].map(row => [row.querySelector('dt')?.textContent ?? '', row.querySelector('dd')?.textContent ?? '']));
			assert.deepStrictEqual({ id: facts['Device ID'], computer: facts.Computer, system: facts.System, addresses: facts.Addresses }, { id: 'aaaaaaaa…aaaaaa', computer: 'studio.local', system: 'macOS', addresses: '2 direct, 1 relay' });
		});
		test('the full device id is reachable', () => {
			assert.strictEqual(page.querySelector('.device-id')?.getAttribute('title'), STUDIO);
			assert.strictEqual(page.querySelector('.device-copy')?.getAttribute('aria-label'), 'Copy device ID');
		});
		test('Copy device ID puts the full id on the clipboard', async () => {
			click(page.querySelector<HTMLButtonElement>('.device-copy'));
			await settled();
			assert.strictEqual(fixture.clipboard.written.at(-1), STUDIO);
		});
		test('the identity line shows presence and system', () => {
			assert.strictEqual(page.querySelector('.device-panel-state')?.textContent, 'Connected · macOS');
		});
		test('a missing host name is shown as not reported', async () => {
			click(fixture.row(1));
			await settled();
			assert.strictEqual(page.querySelector('.device-facts dd:not(:has(code))')?.textContent, 'Not reported');
		});
		test('a disconnected computer offers no Connect button', () => {
			click(fixture.row(1));
			assert.ok(![...page.querySelectorAll('.device-panel button')].some(button => button.textContent === 'Connect'));
		});
	});
	suite('rename and forget', () => {
		const renameAction = (): HTMLButtonElement => {
			const button = page.querySelector<HTMLButtonElement>('.device-panel .device-text-button:not(.device-forget)');
			assert.strictEqual(button?.textContent, 'Rename…');
			return button;
		};
		const field = (): HTMLInputElement => {
			const input = page.querySelector<HTMLInputElement>('.device-rename input');
			assert.ok(input);
			return input;
		};
		function type(name: string): void {
			field().value = name;
			field().dispatchEvent(new Event('input'));
			pressKey(field(), 'Enter', 13);
		}
		setup(() => {
			click(fixture.row(1));
		});
		test('the name field is hidden until Rename…', () => {
			assert.strictEqual(page.querySelector<HTMLElement>('.device-rename')?.hidden, true);
		});
		test('Rename… edits the name in place', () => {
			click(renameAction());
			assert.strictEqual(page.querySelector<HTMLElement>('.device-rename')?.hidden, false);
			assert.strictEqual(page.querySelector<HTMLElement>('.device-panel h2')?.hidden, true);
		});
		test('Rename… focuses the current name', () => {
			click(renameAction());
			assert.strictEqual(document.activeElement, field());
			assert.strictEqual(field().value, 'Lab Laptop');
		});
		test('Escape in the name field closes it, keeps the details open and returns focus to Rename…', () => {
			click(renameAction());
			pressKey(field(), 'Escape', 27);
			assert.strictEqual(page.querySelector<HTMLElement>('.device-rename')?.hidden, true);
			assert.strictEqual(panel()?.hidden, false);
			assert.strictEqual(document.activeElement, renameAction());
		});
		test('a saved name is rendered as text, not markup', async () => {
			click(renameAction());
			type('<b>Personal</b>');
			await settled();
			assert.ok(fixture.row(1).textContent.includes('<b>Personal</b>'));
			assert.strictEqual(page.querySelector('b'), null);
		});
		test('a saved name closes the field', async () => {
			click(renameAction());
			type('Personal');
			await settled();
			assert.strictEqual(page.querySelector<HTMLElement>('.device-rename')?.hidden, true);
		});
		test('a late rename failure from the previous selection is not shown', async () => {
			let reject: (reason: Error) => void = () => undefined;
			fixture.renameResult = new Promise<void>((_resolve, fail) => {
				reject = fail;
			});
			click(renameAction());
			type('Pending');
			click(fixture.row(0));
			reject(new Error('Old device failure'));
			await settled();
			assert.ok(!panel()?.textContent.includes('Old device failure'));
		});
		test('Forget removes nothing before it is confirmed', () => {
			click(page.querySelector<HTMLElement>('.device-forget'));
			assert.strictEqual(fixture.devices.length, 3);
		});
		test('a confirmed Forget removes the device and closes its details', async () => {
			click(page.querySelector<HTMLElement>('.device-forget'));
			click(page.querySelector('.device-forget-confirm .monaco-button:last-child'));
			await settled();
			assert.deepStrictEqual(fixture.devices.map(device => device.id), [STUDIO, OFFICE]);
			assert.strictEqual(panel()?.hidden, true);
		});
	});
	suite('pairing buttons', () => {
		const heading = (): HTMLButtonElement[] => [...page.querySelectorAll<HTMLButtonElement>('.devices-heading .device-pairing-button')];
		const choices = (target: DevicePageFixture): HTMLButtonElement[] => [...target.page.element.querySelectorAll<HTMLButtonElement>('.device-empty .device-pairing-choice')];
		test('a device list offers inviting and opening beside its heading', () => {
			assert.deepStrictEqual(heading().map(button => button.textContent), ['Invite another computer', 'Open an invitation']);
			assert.ok(heading().every(shown));
		});
		test('a device list does not show the empty page\'s choices', () => {
			assert.strictEqual(shown(page.querySelector('.device-pairing-choices')), false);
		});
		test('each heading button runs its command from itself', () => {
			const [create, open] = heading();
			click(create);
			click(open);
			const ran = fixture.commands.map(([id, context]) => [id, (context as { anchor?: unknown } | undefined)?.anchor]);
			assert.deepStrictEqual(ran, [[CreateInvitationAction.ID, create], [ImportInvitationAction.ID, open]]);
		});
		test('pairing is not offered without the engine, and returns with it', () => {
			fixture.engineAvailable.set(false);
			assert.deepStrictEqual(heading().map(button => button.disabled), [true, true]);
			fixture.engineAvailable.set(true);
			assert.deepStrictEqual(heading().map(button => button.disabled), [false, false]);
		});
		test('an empty page offers two choices, inviting first', () => {
			const empty = disposables.add(new DevicePageFixture('empty'));
			assert.deepStrictEqual(choices(empty).map(choice => [shown(choice), choice.classList.contains('primary')]), [[true, true], [true, false]]);
		});
		test('an empty page shows no heading buttons', () => {
			const empty = disposables.add(new DevicePageFixture('empty'));
			assert.strictEqual(shown(empty.page.element.querySelector('.devices-heading .device-pairing-actions')), false);
		});
		test('an empty page\'s choices sit outside its status announcement', () => {
			const empty = disposables.add(new DevicePageFixture('empty'));
			assert.strictEqual(empty.page.element.querySelector('[role="status"] .device-pairing-choice'), null);
		});
		test('an empty page\'s choice runs its command', () => {
			const empty = disposables.add(new DevicePageFixture('empty'));
			click(choices(empty)[1]);
			assert.strictEqual(empty.commands.at(-1)?.[0], ImportInvitationAction.ID);
		});
		test('an invitation opened from outside shows under the visible Open an invitation button', async () => {
			await fixture.popoverPairing.openInvitation(new File([new Uint8Array([1])], 'Lab.vnl'));
			assert.strictEqual(fixture.popover.isOpen, true);
			fixture.popover.hide();
		});
		test('a closed invitation stays closed while its step changes only its message', async () => {
			await fixture.popoverPairing.openInvitation(new File([new Uint8Array([1])], 'Lab.vnl'));
			fixture.popover.hide();
			await fixture.popoverPairing.submitCode('12');
			assert.strictEqual(fixture.popover.isOpen, false);
		});
		test('no pairing choices are offered while devices are unavailable', () => {
			const unavailable = disposables.add(new DevicePageFixture('unavailable'));
			assert.strictEqual(shown(unavailable.page.element.querySelector('.device-pairing-choices')), false);
		});
	});
});

suite('DevicesPage screen', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let snapshots: TestScreenSnapshotService;
	let fixture: DevicePageFixture;
	let page: HTMLElement;
	const caption = (): string | null | undefined => page.querySelector('.device-screen figcaption')?.textContent;
	const frame = (): string | undefined => page.querySelector<HTMLElement>('.device-screen-frame')?.dataset.state;
	const image = (): HTMLImageElement | null => page.querySelector<HTMLImageElement>('.device-screen img');
	const refresh = (): HTMLButtonElement => {
		const button = page.querySelector<HTMLButtonElement>('.device-screen-refresh');
		assert.ok(button);
		return button;
	};
	const button = (label: string): HTMLElement | undefined => [...page.querySelectorAll<HTMLElement>('.device-screen .monaco-button')].find(candidate => candidate.textContent === label);
	const offline = 'Offline. It may be asleep, off, or not running VniLink.';

	/** Selects Studio and answers its request with `result`. */
	async function selectStudio(result = screenshot(Date.now(), 'studio')): Promise<void> {
		click(fixture.row(0));
		snapshots.release(result);
		await settled();
	}
	setup(() => {
		snapshots = new TestScreenSnapshotService();
		snapshots.hold = true;
		fixture = disposables.add(new DevicePageFixture('populated', snapshots));
		page = fixture.page.element;
	});
	test('a disconnected device requests no screenshot and says it is offline', () => {
		click(fixture.row(1));
		assert.deepStrictEqual(snapshots.calls, []);
		assert.strictEqual(caption(), offline);
	});
	test('an offline device shows no screen and offers no Refresh', () => {
		click(fixture.row(1));
		assert.strictEqual(frame(), 'offline');
		assert.strictEqual(refresh().hidden, true);
	});
	test('a reconnecting device is shown reconnecting', () => {
		click(fixture.row(2));
		assert.strictEqual(caption(), 'Reconnecting…');
		assert.strictEqual(frame(), 'connecting');
	});
	test('a device that stopped accepting connections says so and requests no screenshot', () => {
		click(fixture.row(2));
		fixture.setPresence(OFFICE, 'notAccepting');
		assert.strictEqual(caption(), 'Its owner stopped accepting connections.');
		assert.strictEqual(frame(), 'notAccepting');
		assert.deepStrictEqual(snapshots.calls, []);
	});
	test('a connected device shows no picture before the remote image arrives', () => {
		click(fixture.row(0));
		assert.deepStrictEqual(snapshots.calls, [STUDIO]);
		assert.strictEqual(image()?.getAttribute('src'), null);
	});
	test('Refresh is labelled and disabled while a screenshot loads', () => {
		click(fixture.row(0));
		assert.deepStrictEqual({ label: refresh().getAttribute('aria-label'), title: refresh().title, disabled: refresh().disabled, hidden: refresh().hidden }, { label: 'Refresh screen', title: 'Refresh screen', disabled: true, hidden: false });
	});
	test('a late screenshot from the previous computer is not shown on the next', async () => {
		click(fixture.row(0));
		click(fixture.row(1));
		snapshots.release(screenshot(Date.now(), 'late'));
		await settled();
		assert.notStrictEqual(image()?.hasAttribute('src'), true);
	});
	test('a screenshot shows as a described image', async () => {
		await selectStudio(screenshot(Date.now() - 5_000, 'studio'));
		assert.deepStrictEqual({ hidden: image()?.hidden, alt: image()?.alt, url: image()?.src.startsWith('blob:') }, { hidden: false, alt: 'Screenshot of Studio Mac', url: true });
	});
	test('a screenshot is not an input surface', async () => {
		await selectStudio();
		assert.notStrictEqual(image()?.parentElement?.tagName, 'BUTTON');
	});
	test('a screenshot says how long ago it was captured', async () => {
		await selectStudio(screenshot(Date.now() - 5_000, 'studio'));
		assert.strictEqual(caption(), 'Screen captured just now.');
	});
	test('leaving the connected device hides its image', async () => {
		await selectStudio();
		click(fixture.row(2));
		await settled();
		assert.strictEqual(image()?.hidden, true);
	});
	test('a computer that has not allowed capture says so and offers to allow it', async () => {
		await selectStudio({ status: 'needsPermission' });
		assert.strictEqual(caption(), 'That computer has not allowed screen capture.');
		assert.strictEqual(button('Allow screen capture')?.hidden, false);
	});
	test('Refresh takes keyboard focus', async () => {
		await selectStudio({ status: 'needsPermission' });
		refresh().focus();
		assert.strictEqual(document.activeElement, refresh());
	});
	test('only the status is announced, not the capture age', async () => {
		await selectStudio({ status: 'needsPermission' });
		assert.strictEqual(page.querySelector('.device-screen-status')?.getAttribute('role'), 'status');
		assert.strictEqual(page.querySelector('.device-screen-age')?.hasAttribute('role'), false);
		assert.strictEqual(page.querySelector('.device-screen figcaption')?.hasAttribute('role'), false);
	});
	test('Allow screen capture asks for consent and waits for it', async () => {
		await selectStudio({ status: 'needsPermission' });
		click(button('Allow screen capture'));
		assert.strictEqual(snapshots.prompts.at(-1), true);
		assert.strictEqual(page.querySelector('.device-screen .monaco-button')?.classList.contains('disabled'), true);
	});
	test('a cancelled screenshot enables Refresh again and says the screen could not be captured', async () => {
		await selectStudio({ status: 'needsPermission' });
		click(button('Allow screen capture'));
		snapshots.release({ status: 'cancelled' });
		await settled();
		assert.strictEqual(refresh().disabled, false);
		assert.strictEqual(caption(), 'The screen could not be captured.');
	});
	test('a computer that needs a screen chosen offers Choose screen, which asks for one', async () => {
		await selectStudio({ status: 'cancelled' });
		click(refresh());
		snapshots.release({ status: 'needsSourceSelection' });
		await settled();
		assert.strictEqual(caption(), 'Choose a screen on that computer, then refresh.');
		click(button('Choose screen'));
		assert.strictEqual(snapshots.prompts.at(-1), true);
	});
	test('a disconnect hides the screen and says the computer is offline', async () => {
		await selectStudio();
		fixture.setPresence(STUDIO, 'disconnected');
		await settled();
		assert.strictEqual(image()?.hidden, true);
		assert.strictEqual(caption(), offline);
	});
	test('returning to connected refreshes without prompting and shows the new screenshot', async () => {
		await selectStudio();
		fixture.setPresence(STUDIO, 'disconnected');
		await settled();
		const requests = snapshots.calls.length;
		fixture.setPresence(STUDIO, 'connected');
		await settled();
		assert.strictEqual(snapshots.calls.length, requests + 1);
		assert.strictEqual(snapshots.prompts.at(-1), false);
		snapshots.release(screenshot(Date.now(), 'again'));
		await settled();
		assert.strictEqual(image()?.hidden, false);
	});
	test('a replaced connection drops the previous screenshot and shows the new one without prompting', async () => {
		await selectStudio();
		const requests = snapshots.calls.length;
		snapshots.accessResult = { status: 'allowed', endpointId: STUDIO, sessionId: '2', generation: '9' };
		fixture.update(fixture.devices);
		await settled();
		assert.strictEqual(snapshots.calls.length, requests + 1);
		assert.strictEqual(snapshots.prompts.at(-1), false);
		assert.strictEqual(image()?.hidden, true);
		snapshots.release(screenshot(Date.now(), 'replaced', '2', '9'));
		await settled();
		assert.strictEqual(image()?.hidden, false);
	});
	test('a revoked view drops the screenshot and says viewing is turned off', async () => {
		await selectStudio();
		snapshots.accessResult = { status: 'denied' };
		fixture.update(fixture.devices);
		await settled();
		assert.strictEqual(image()?.hidden, true);
		assert.strictEqual(caption(), 'Screen viewing is turned off on that computer.');
	});
});
