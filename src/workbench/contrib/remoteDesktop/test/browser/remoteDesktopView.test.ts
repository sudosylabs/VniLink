// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { isMacintosh } from '../../../../../base/common/platform.js';
import { animationFrames } from '../../../../../base/test/browser/waits.js';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../../base/test/common/utils.js';
import type { DesktopRateLimit } from '../../../../../platform/engine/common/dataChannel.js';
import type { DesktopInput } from '../../../../services/remoteDesktop/remoteDesktopService.js';
import { shownSize } from '../../pictureSize.js';
import { RemoteDesktopView } from '../../remoteDesktopView.js';
import type { TestDesktopSession } from './desktopViewFakes.js';
import { click, createTabFixture, DesktopTab, type RemoteDesktopTabFixture } from './remoteDesktopTabFixture.js';

/** A key the session sends: where it is, whether it is down, what it types, and whether it composes. */
function key(usage: number, down: boolean, text = '', composing = false): DesktopInput {
	return { type: 'submitLocalKey', usage, down, text, composing };
}

suite('RemoteDesktopView', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let fixture: RemoteDesktopTabFixture<TestDesktopSession>;
	setup(() => {
		fixture = disposables.add(createTabFixture());
	});

	/** Opens a tab and shows its first picture. */
	async function live(control = true): Promise<{ tab: DesktopTab; session: TestDesktopSession }> {
		const tab = new DesktopTab(fixture.open(control), fixture.contextMenu);
		await fixture.opened(1);
		const session = fixture.sessions[0];
		session.picture();
		await tab.when(() => tab.overlay.hidden === true && tab.role() === (control ? 'Controlling' : 'Viewing'));
		return { tab, session };
	}

	/** Shows a picture and lets two frames draw it. */
	async function show(session: TestDesktopSession, ...picture: Parameters<TestDesktopSession['picture']>): Promise<void> {
		session.picture(...picture);
		await animationFrames(2);
	}
	suite('opening', () => {
		test('Remote Desktop opens a controlling tab named for the computer', () => {
			const view = fixture.open();
			assert.ok(view instanceof RemoteDesktopView);
			assert.deepStrictEqual({ id: view.id, label: view.label, control: view.control }, { id: 'desktop:studio', label: 'Studio PC', control: true });
		});
		test('the tab has a screen and nothing below it', () => {
			const tab = new DesktopTab(fixture.open(), fixture.contextMenu);
			assert.ok(tab.overlay);
			assert.ok(tab.canvas);
			assert.strictEqual(tab.element.querySelector('.desktop-status, footer'), null);
		});
		test('the tab says it is connecting until the first picture', async () => {
			const tab = new DesktopTab(fixture.open(), fixture.contextMenu);
			await fixture.opened(1);
			assert.strictEqual(tab.overlay.hidden, false);
			assert.ok(tab.overlay.textContent.includes('Connecting to Studio PC'), tab.overlay.textContent);
		});
		test('while the other computer\'s person is asked, the tab says whom it waits for', async () => {
			const tab = new DesktopTab(fixture.open(), fixture.contextMenu);
			await fixture.opened(1);
			fixture.devices.setAwaiting('studio', true);
			assert.ok(tab.overlay.textContent.includes('Waiting for Studio PC to accept'), tab.overlay.textContent);
			fixture.devices.setAwaiting('studio', false);
			assert.ok(tab.overlay.textContent.includes('Connecting to Studio PC'), tab.overlay.textContent);
		});
		test('the first picture is drawn at its size and takes the keyboard', async () => {
			const { tab } = await live();
			assert.deepStrictEqual([tab.canvas.width, tab.canvas.height], [1920, 1080]);
			assert.strictEqual(document.activeElement, tab.canvas);
		});
		test('the controls do not cover the first picture', async () => {
			const { tab } = await live();
			assert.strictEqual(tab.required('.desktop-bar-host').classList.contains('revealed'), false);
		});
		test('the bar names the display that shows', async () => {
			const { tab } = await live();
			await tab.when(() => tab.control('Display').getAttribute('aria-label') === 'Display: Display 1');
		});
	});
	suite('pictures', () => {
		test('of pictures decoded within one frame, only the newest is drawn', async () => {
			const { tab, session } = await live();
			const presented = session.presentedPictures.length;
			session.picture();
			session.picture();
			session.picture();
			await tab.when(() => session.presentedPictures.length === presented + 1);
			await animationFrames(1);
			assert.deepStrictEqual({ drawn: session.presentedPictures.length - presented, skipped: session.skippedPictures.length }, { drawn: 1, skipped: 2 });
		});

		/** A still over a repeat of the same grey screen, as the stream sends them. */
		async function still(): Promise<{ tab: DesktopTab; session: TestDesktopSession }> {
			const shown = await live();
			await show(shown.session, 'unchanged', '#808080');
			await show(shown.session, 'still', '#ff8000');
			return shown;
		}
		test('a still is drawn', async () => {
			const { tab } = await still();
			assert.strictEqual(tab.shownColour(), '255,128,0');
		});
		test('a picture of the same screen is not drawn over a still', async () => {
			const { tab, session } = await still();
			await show(session, 'unchanged', '#808080');
			assert.strictEqual(tab.shownColour(), '255,128,0');
		});
		test('a small change redraws only its tiles, keeping the still elsewhere', async () => {
			const { tab, session } = await still();
			await show(session, 'changed', '#808080', '#00ff00');
			assert.deepStrictEqual([tab.shownColour(110, 110), tab.shownColour(), tab.shownColour(300, 300)], ['0,255,0', '255,128,0', '255,128,0']);
		});
		test('a repeat after a small change keeps both the change and the still', async () => {
			const { tab, session } = await still();
			await show(session, 'changed', '#808080', '#00ff00');
			await show(session, 'unchanged', '#808080', '#00ff00');
			assert.deepStrictEqual([tab.shownColour(110, 110), tab.shownColour()], ['0,255,0', '255,128,0']);
		});
		test('a changed screen replaces the still everywhere', async () => {
			const { tab, session } = await still();
			await show(session, 'changed', '#0000ff');
			assert.strictEqual(tab.shownColour(), '0,0,255');
		});
		test('without repeats, the changed picture before a still is what a tick is compared with', async () => {
			const { tab, session } = await live();
			await show(session, 'changed', '#808080');
			await show(session, 'still', '#ff8000');
			await show(session, 'changed', '#808080', '#00ff00');
			assert.deepStrictEqual([tab.shownColour(110, 110), tab.shownColour()], ['0,255,0', '255,128,0']);
		});
		test('a picture that says where it changed redraws only there', async () => {
			const { tab, session } = await still();
			await show(session, 'changed', '#707070', '#ff00ff', [{ x: 96, y: 96, width: 64, height: 64 }]);
			assert.deepStrictEqual([tab.shownColour(110, 110), tab.shownColour(), tab.shownColour(300, 300)], ['255,0,255', '255,128,0', '255,128,0']);
		});
	});
	suite('picture details', () => {
		test('the line gives frame rate, round trip and path', async () => {
			const { tab } = await live();
			await tab.when(() => tab.text('.desktop-stats-summary') === '60 fps · 18 ms · Relay');
		});
		test('the details open on request', async () => {
			const { tab } = await live();
			await tab.when(() => tab.text('.desktop-stats-summary') !== '');
			const toggle = tab.required('.desktop-stats-toggle');
			assert.strictEqual(toggle.getAttribute('aria-expanded'), 'false');
			click(toggle);
			assert.strictEqual(toggle.getAttribute('aria-expanded'), 'true');
		});

		/** A live tab with its details open. */
		async function details(): Promise<{ tab: DesktopTab; session: TestDesktopSession }> {
			const shown = await live();
			await shown.tab.when(() => shown.tab.text('.desktop-stats-summary') !== '');
			click(shown.tab.required('.desktop-stats-toggle'));
			return shown;
		}
		test('the bitrate says it was lowered for the network', async () => {
			const { tab } = await details();
			await tab.when(() => tab.detail('Bitrate') === '9.4 Mb/s · Lowered for the network');
		});
		test('the details describe the picture, codec, decoder, encoder and connection, with the colour a still brought', async () => {
			const { tab, session } = await details();
			await show(session, 'still', '#ff8000');
			await tab.when(() => tab.detail('Codec') === 'H.264 · 4:2:0, stills 4:4:4');
			assert.deepStrictEqual(['Picture', 'Codec', 'Decoder', 'Encoder', 'Connection'].map(term => tab.detail(term)), [
				'1920 × 1080 · 60 fps', 'H.264 · 4:2:0, stills 4:4:4', 'Hardware, in this window', 'Hardware, on Studio PC', 'Relay · 18 ms round trip',
			]);
		});
		test('the details give the time spent on the other computer and on the network', async () => {
			const { tab } = await details();
			await tab.when(() => tab.text('.desktop-stats-latency') !== '');
			const latency = tab.text('.desktop-stats-latency');
			assert.ok(latency.includes('On Studio PC9.0 · 14.0 ms') && latency.includes('Network9 ms each way'), latency);
		});
		for (const [limit, held] of [['setting', 'At your bandwidth limit'], ['relay', 'Limited by the relay'], ['network', 'Limited by the network'], ['encoder', 'Limited by Studio PC'], ['receiver', 'Limited by this computer']] as const satisfies readonly (readonly [DesktopRateLimit, string])[]) {
			test(`a steady bitrate held by the ${limit} says "${held}"`, async () => {
				const { tab, session } = await details();
				session.reason = 'stable';
				session.limit = limit;
				await tab.when(() => tab.detail('Bitrate') === `9.4 Mb/s · ${held}`);
			});
		}
		test('a bitrate nothing holds is not explained', async () => {
			const { tab, session } = await details();
			session.reason = 'stable';
			await tab.when(() => tab.detail('Bitrate') === '9.4 Mb/s');
		});
		test('a lowered bitrate says its cause once', async () => {
			const { tab, session } = await details();
			session.limit = 'network';
			await tab.when(() => tab.detail('Bitrate') !== '');
			await tab.when(() => tab.detail('Bitrate') === '9.4 Mb/s · Lowered for the network');
		});
		test('the worst condition shows under the line', async () => {
			const { tab } = await live();
			const condition = tab.required('.desktop-stats-condition');
			await tab.when(() => !condition.hidden);
			assert.strictEqual(condition.textContent, 'Slow network: lowered quality');
		});
		test('every condition is listed on hover', async () => {
			const { tab } = await live();
			const condition = tab.required('.desktop-stats-condition');
			await tab.when(() => !condition.hidden);
			assert.strictEqual(condition.title, 'Slow network: lowered quality\nConnected through a relay');
		});
		test('a relay alone is not shown as a problem', async () => {
			const { tab, session } = await live();
			const condition = tab.required('.desktop-stats-condition');
			await tab.when(() => !condition.hidden);
			session.conditions = ['relay.path'];
			await tab.when(() => condition.hidden === true);
		});
		test('encoding in software there is explained', async () => {
			const { tab, session } = await live();
			session.conditions = ['encoder.software'];
			const condition = tab.required('.desktop-stats-condition');
			await tab.when(() => condition.textContent === 'Studio PC is encoding in software');
			assert.ok(condition.classList.contains('explanation'));
		});
		test('a direct path is said', async () => {
			const { tab, session } = await live();
			session.conditions = ['encoder.software'];
			await tab.when(() => tab.text('.desktop-stats-summary') === '60 fps · 18 ms · Direct');
		});
		test('the details can be put away, and that is remembered', async () => {
			const { tab } = await live();
			tab.pick(tab.control('More'), 'Show Picture Details');
			assert.ok(tab.required('.desktop-stats').classList.contains('details-off'));
			assert.strictEqual(fixture.uiState.values.get('desktop.showStats'), false);
		});
	});
	suite('details placement', () => {
		test('the details start top right', async () => {
			const { tab } = await live();
			assert.strictEqual(tab.required('.desktop-stats').dataset.corner, 'topRight');
		});
		/** Drags the details by their grip to the bottom left of the tab. */
		function drag(tab: DesktopTab): void {
			const stats = tab.required('.desktop-stats');
			const grip = tab.required('.desktop-stats-grip');
			const viewBox = tab.element.getBoundingClientRect();
			const gripBox = grip.getBoundingClientRect();
			// The test window has no pointer to capture; the drag's events come to the overlay as a captured pointer's would.
			stats.setPointerCapture = () => undefined;
			stats.releasePointerCapture = () => undefined;
			grip.dispatchEvent(new PointerEvent('pointerdown', { clientX: gripBox.left + 4, clientY: gripBox.top + 4, button: 0, pointerId: 7, bubbles: true }));
			stats.dispatchEvent(new PointerEvent('pointermove', { clientX: viewBox.left + 40, clientY: viewBox.bottom - 40, pointerId: 7, bubbles: true }));
			stats.dispatchEvent(new PointerEvent('pointerup', { clientX: viewBox.left + 40, clientY: viewBox.bottom - 40, pointerId: 7, bubbles: true }));
		}
		test('dragged, the details settle in the nearest corner, remembered for the computer', async () => {
			const { tab } = await live();
			drag(tab);
			assert.strictEqual(tab.required('.desktop-stats').dataset.corner, 'bottomLeft');
			assert.strictEqual(fixture.uiState.values.get('desktop.statsCorner.studio'), 'bottomLeft');
		});
		test('a capture the window refuses still lets the drag move the details', async () => {
			const { tab } = await live();
			const stats = tab.required('.desktop-stats');
			const grip = tab.required('.desktop-stats-grip');
			const viewBox = tab.element.getBoundingClientRect();
			const gripBox = grip.getBoundingClientRect();
			stats.setPointerCapture = () => {
				throw new DOMException('The element has no pointer capture');
			};
			grip.dispatchEvent(new PointerEvent('pointerdown', { clientX: gripBox.left + 4, clientY: gripBox.top + 4, button: 0, pointerId: 7, bubbles: true }));
			// A refused capture delivers the rest of the drag to the window.
			window.dispatchEvent(new PointerEvent('pointermove', { clientX: viewBox.left + 40, clientY: viewBox.bottom - 40, pointerId: 7, bubbles: true }));
			assert.ok(stats.classList.contains('dragging'));
			window.dispatchEvent(new PointerEvent('pointerup', { clientX: viewBox.left + 40, clientY: viewBox.bottom - 40, pointerId: 7, bubbles: true }));
			assert.strictEqual(stats.dataset.corner, 'bottomLeft');
		});
		test('the arrow keys move the details between corners from the grip', async () => {
			const { tab } = await live();
			drag(tab);
			tab.required('.desktop-stats-grip').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true, cancelable: true }));
			assert.strictEqual(tab.required('.desktop-stats').dataset.corner, 'topLeft');
		});
	});
	suite('keyboard', () => {
		test('the session opens by letting go of held keys and saying whether Control and Command are exchanged', async () => {
			// By default with this Windows PC from a Mac, not from another PC.
			const { session } = await live();
			assert.deepStrictEqual(session.sent.slice(0, 2), [{ type: 'releaseSessionInput' }, { type: 'swapSessionModifiers', enabled: isMacintosh }]);
		});
		test('the shown display is selected before any input is sent', async () => {
			const { session } = await live();
			// A focused window also asks to capture system keys, in an order of its own.
			const selected = session.sent.findIndex(input => input.type === 'selectDisplays');
			assert.deepStrictEqual(session.sent[selected], { type: 'selectDisplays', ids: ['7'] });
			assert.ok(!session.sent.slice(0, selected).some(input => input.type.startsWith('submit')), 'input was sent before the display was selected');
		});
		test('keys go by where they are with what they type; a held key repeats, a shortcut types nothing, a dead key composes', async () => {
			const { tab, session } = await live();
			const canvas = tab.canvas;
			tab.press(canvas, 'keydown', 'KeyA', 'a');
			tab.press(canvas, 'keydown', 'KeyA', 'a', { repeat: true });
			tab.press(canvas, 'keyup', 'KeyA', 'a');
			tab.press(canvas, 'keydown', 'KeyC', 'c', { metaKey: true });
			tab.press(canvas, 'keyup', 'KeyC', 'c');
			tab.press(canvas, 'keydown', 'BracketLeft', 'Dead');
			tab.press(canvas, 'keyup', 'BracketLeft', 'Dead');
			assert.deepStrictEqual(session.sent.filter(input => input.type === 'submitLocalKey'), [
				key(4, true, 'a'), key(4, true, 'a'), key(4, false),
				key(6, true), key(6, false),
				key(0x2f, true, '', true), key(0x2f, false),
			]);
		});
		(isMacintosh ? test : test.skip)('on a Mac, the characters Option makes are typed, as Option-à types @ on a French keyboard', async () => {
			const { tab, session } = await live();
			tab.press(tab.canvas, 'keydown', 'Digit0', '@', { altKey: true });
			assert.deepStrictEqual(session.sent.at(-1), key(0x27, true, '@'));
		});
		test('with focus, the picture takes the keyboard and this computer\'s shortcuts', async () => {
			// A test window has no focus of its own to lose, so focus and blur are dispatched as the system would.
			const { tab, session } = await live();
			tab.canvas.dispatchEvent(new FocusEvent('focus'));
			assert.strictEqual(session.sent.filter(input => input.type === 'captureSystemKeys').at(-1)?.enabled, true);
			assert.strictEqual(fixture.nativeHost.remoteKeyboard.at(-1), true);
		});
		test('leaving the screen gives the keyboard back and lets go of held keys', async () => {
			const { tab, session } = await live();
			tab.canvas.dispatchEvent(new FocusEvent('focus'));
			tab.press(tab.canvas, 'keydown', 'ShiftLeft', 'Shift', { shiftKey: true });
			tab.canvas.dispatchEvent(new FocusEvent('blur'));
			assert.deepStrictEqual(session.sent.slice(-2), [{ type: 'captureSystemKeys', enabled: false }, { type: 'releaseSessionInput' }]);
		});
		test('leaving the screen gives the window its shortcuts back', async () => {
			const { tab } = await live();
			tab.canvas.dispatchEvent(new FocusEvent('focus'));
			tab.canvas.dispatchEvent(new FocusEvent('blur'));
			assert.strictEqual(fixture.nativeHost.remoteKeyboard.at(-1), false);
		});
	});
	suite('pointer', () => {
		/** Where the fitted picture is, and its middle as a pointer event takes it. */
		async function fitted(): Promise<{ tab: DesktopTab; session: TestDesktopSession; box: DOMRect; center: PointerEventInit }> {
			const shown = await live();
			const box = shown.tab.canvas.getBoundingClientRect();
			const center = { clientX: box.left + box.width / 2, clientY: box.top + box.height / 2, bubbles: true, cancelable: true, pointerId: 1 };
			return { ...shown, box, center };
		}
		test('a fitted picture shows its own pixels, smaller when the tab cannot hold it', async () => {
			const { tab, box } = await fitted();
			const ratio = window.devicePixelRatio || 1;
			const expected = shownSize('fit', 1920, 1080, { width: tab.stage.clientWidth, height: tab.stage.clientHeight }, ratio);
			assert.ok(Math.abs(box.width - expected.width) < 0.5 && Math.abs(box.height - expected.height) < 0.5, `${String(box.width)} × ${String(box.height)}`);
			assert.ok(expected.width * ratio <= 1920);
		});
		test('a fitted picture is centred', async () => {
			const { tab, box } = await fitted();
			const stage = tab.stage.getBoundingClientRect();
			assert.ok(Math.abs(box.left - stage.left - (tab.stage.clientWidth - box.width) / 2) < 1);
			assert.ok(Math.abs(box.top - stage.top - (tab.stage.clientHeight - box.height) / 2) < 1);
		});
		test('the pointer lands on the other display through the picture as shown', async () => {
			const { tab, session, center } = await fitted();
			tab.canvas.dispatchEvent(new PointerEvent('pointermove', center));
			const pointer = session.sent.find(input => input.type === 'submitPointer');
			// At a fractional scale the middle of the shown picture falls between two of its pixels.
			assert.ok(pointer?.type === 'submitPointer' && Math.abs(pointer.x - (1920 + 960)) <= 1 && Math.abs(pointer.y - 540) <= 1 && pointer.display === '7' && pointer.revision === '3', JSON.stringify(pointer));
		});
		test('a click reaches the other computer', async () => {
			const { tab, session, center } = await fitted();
			tab.canvas.dispatchEvent(new PointerEvent('pointermove', center));
			tab.canvas.dispatchEvent(new PointerEvent('pointerdown', { ...center, button: 0 }));
			tab.canvas.dispatchEvent(new PointerEvent('pointerup', { ...center, button: 0 }));
			assert.deepStrictEqual(session.sent.filter(input => input.type === 'submitButton'), [
				{ type: 'submitButton', button: 'Left', down: true }, { type: 'submitButton', button: 'Left', down: false },
			]);
		});
		test('the pointer above the picture, as a drag captured from it reports, is not sent', async () => {
			const { tab, session, center, box } = await fitted();
			const before = session.sent.length;
			tab.canvas.dispatchEvent(new PointerEvent('pointermove', { ...center, clientY: box.top - 2 }));
			assert.strictEqual(session.sent.length, before);
		});
		test('a notch of the wheel is 120 of the engine\'s units', async () => {
			const { tab, session } = await fitted();
			tab.canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: 100, bubbles: true, cancelable: true }));
			assert.deepStrictEqual(session.sent.at(-1), { type: 'submitPreciseWheel', dx: 0, dy: 120 });
		});
		test('over the picture, this pointer takes the other pointer\'s shape', async () => {
			const { tab, session } = await fitted();
			tab.canvas.dispatchEvent(new MouseEvent('mouseenter'));
			session.pointer(960, 540);
			assert.ok(tab.canvas.style.cursor.includes('image-set'), tab.canvas.style.cursor);
			assert.strictEqual(tab.mark.hidden, true);
		});
		test('away from the picture, the other pointer is drawn where it is', async () => {
			const { tab, session, box } = await fitted();
			tab.canvas.dispatchEvent(new MouseEvent('mouseenter'));
			session.pointer(960, 540);
			tab.canvas.dispatchEvent(new MouseEvent('mouseleave'));
			assert.strictEqual(tab.mark.hidden, false);
			const scale = box.width / 1920;
			const placed = tab.mark.getBoundingClientRect();
			assert.ok(Math.abs(placed.left - (box.left + 960 * scale)) < 1.5 && Math.abs(placed.top - (box.top + 540 * scale)) < 1.5, `${String(placed.left)}, ${String(placed.top)}`);
		});
		test('a hidden other pointer shows nowhere', async () => {
			const { tab, session } = await fitted();
			session.pointer(960, 540);
			session.pointer(960, 540, false);
			assert.strictEqual(tab.mark.hidden, true);
			assert.strictEqual(tab.canvas.style.cursor, 'none');
		});
	});
	suite('picture size', () => {
		test('the bar offers each size and full screen, marking the one in use', async () => {
			const { tab } = await live();
			const sizes = tab.control('Picture size');
			assert.strictEqual(sizes.getAttribute('aria-label'), 'Picture size: Fit');
			click(sizes);
			assert.strictEqual(fixture.contextMenu.lastMenu().join(), 'Fit,Stretch,-,50%,75%,Original size,150%,200%,-,Full Screen');
			assert.strictEqual(fixture.contextMenu.menus.at(-1)?.actions[0].checked, true);
		});
		test('the bar says the size chosen', async () => {
			const { tab } = await live();
			tab.pick(tab.control('Picture size'), 'Original size');
			assert.strictEqual(tab.control('Picture size').getAttribute('aria-label'), 'Picture size: Original size');
		});
		test('original size shows one picture pixel on one screen pixel', async () => {
			const { tab } = await live();
			tab.pick(tab.control('Picture size'), 'Original size');
			const ratio = window.devicePixelRatio || 1;
			const original = tab.canvas.getBoundingClientRect();
			assert.ok(Math.abs(original.width * ratio - 1920) < 0.5 && Math.abs(original.height * ratio - 1080) < 0.5, String(original.width * ratio));
		});
		test('the stage scrolls exactly when the picture is larger than the tab', async () => {
			const { tab } = await live();
			const room = tab.stage.clientWidth;
			tab.pick(tab.control('Picture size'), 'Original size');
			assert.strictEqual(tab.stage.scrollWidth > tab.stage.clientWidth, tab.canvas.getBoundingClientRect().width > room);
		});

		/** Original size, scrolled 200 pixels where the tab is smaller than the picture, and a pointer moved 100 pixels into the view. */
		async function scrolled(): Promise<{ tab: DesktopTab; session: TestDesktopSession; shown: DOMRect; at: PointerEventInit & { clientX: number } }> {
			const { tab, session } = await live();
			tab.pick(tab.control('Picture size'), 'Original size');
			if (tab.stage.scrollWidth > tab.stage.clientWidth) {
				tab.stage.scrollLeft = 200;
				tab.stage.dispatchEvent(new Event('scroll'));
			}
			const shown = tab.canvas.getBoundingClientRect();
			const stage = tab.stage.getBoundingClientRect();
			const at = { clientX: Math.max(shown.left, stage.left) + 100, clientY: Math.max(shown.top, stage.top) + 100, bubbles: true, cancelable: true };
			return { tab, session, shown, at: { ...at, pointerId: 1 } };
		}
		test('the pointer sent is what is under it in a scrolled picture', async () => {
			const { tab, session, shown, at } = await scrolled();
			tab.canvas.dispatchEvent(new PointerEvent('pointermove', at));
			const sent = session.sent.at(-1);
			assert.ok(sent?.type === 'submitPointer' && sent.x === 1920 + Math.floor((at.clientX - shown.left) / shown.width * 1920), JSON.stringify(sent));
		});
		test('the other pointer follows a scrolled picture', async () => {
			const { tab, session, shown } = await scrolled();
			session.pointer(960, 540);
			const mark = tab.mark.getBoundingClientRect();
			assert.ok(Math.abs(mark.left - (shown.left + 960 * shown.width / 1920)) < 1.5, String(mark.left));
		});
		test('stretch fills the tab both ways it can, keeping the picture\'s shape', async () => {
			const { tab } = await live();
			const room = { width: tab.stage.clientWidth, height: tab.stage.clientHeight };
			tab.pick(tab.control('Picture size'), 'Stretch');
			const stretched = tab.canvas.getBoundingClientRect();
			assert.ok(Math.abs(Math.min(room.width - stretched.width, room.height - stretched.height)) < 1, `${String(stretched.width)} × ${String(stretched.height)}`);
			assert.ok(tab.stage.scrollWidth <= tab.stage.clientWidth);
		});
	});
	suite('full screen', () => {
		test('full screen covers the window', async () => {
			const { tab } = await live();
			const toggle = tab.control('Full Screen');
			click(toggle);
			await tab.when(() => fixture.nativeHost.fullScreen.join() === 'true' && tab.element.classList.contains('desktop-view-fullscreen') && toggle.getAttribute('aria-label') === 'Exit Full Screen');
		});
		test('its button leaves full screen', async () => {
			const { tab } = await live();
			const toggle = tab.control('Full Screen');
			click(toggle);
			await tab.when(() => tab.element.classList.contains('desktop-view-fullscreen'));
			click(toggle);
			await tab.when(() => fixture.nativeHost.fullScreen.join() === 'true,false' && !tab.element.classList.contains('desktop-view-fullscreen'));
		});
		test('leaving full screen from the window restores the tab', async () => {
			// Not by a key, which goes to the other computer.
			const { tab } = await live();
			const toggle = tab.control('Full Screen');
			click(toggle);
			await tab.when(() => tab.element.classList.contains('desktop-view-fullscreen'));
			fixture.nativeHost.leaveFullScreen();
			assert.strictEqual(tab.element.classList.contains('desktop-view-fullscreen'), false);
			assert.strictEqual(toggle.getAttribute('aria-label'), 'Full Screen');
		});
	});
	suite('controls', () => {
		test('a moment after its last menu, the bar leaves the picture by itself', async () => {
			const { tab } = await live();
			const bar = tab.required('.desktop-bar-host');
			click(tab.control('Picture size'));
			await tab.when(() => bar.classList.contains('revealed'));
			await tab.when(() => !bar.classList.contains('revealed'));
		});
		test('the top edge brings the controls down', async () => {
			const { tab } = await live();
			const box = tab.element.getBoundingClientRect();
			tab.element.dispatchEvent(new PointerEvent('pointermove', { clientX: box.left + 400, clientY: box.top + 2, bubbles: true }));
			assert.ok(tab.required('.desktop-bar-host').classList.contains('revealed'));
		});
	});
	suite('displays', () => {
		test('the bar lists each display there with its size', async () => {
			const { tab } = await live();
			click(tab.control('Display'));
			assert.strictEqual(fixture.contextMenu.lastMenu().join(), 'Display 1 · 1920 × 1080,Display 2 · 2560 × 1440');
		});
		test('choosing a display asks for it', async () => {
			const { tab, session } = await live();
			tab.pick(tab.control('Display'), 'Display 2 · 2560 × 1440');
			assert.deepStrictEqual(session.sent.at(-1), { type: 'selectDisplays', ids: ['8'] });
		});
		test('a picture of the old display does not take the choice back', async () => {
			const { tab, session } = await live();
			tab.pick(tab.control('Display'), 'Display 2 · 2560 × 1440');
			const picked = session.sent.length;
			await show(session);
			assert.ok(
				!session.sent.slice(picked).some(input => input.type === 'selectDisplays' && input.ids[0] === '7'),
				'a picture of display 7 selected it again',
			);
		});
		test('the new display is named once its picture shows', async () => {
			const { tab, session } = await live();
			tab.pick(tab.control('Display'), 'Display 2 · 2560 × 1440');
			session.picture('changed', '#1d4b6c', undefined, undefined, '8');
			await tab.when(() => tab.control('Display').getAttribute('aria-label') === 'Display: Display 2');
		});
		test('switching back shows the first display', async () => {
			const { tab, session } = await live();
			tab.pick(tab.control('Display'), 'Display 2 · 2560 × 1440');
			session.picture('changed', '#1d4b6c', undefined, undefined, '8');
			await tab.when(() => tab.control('Display').getAttribute('aria-label') === 'Display: Display 2');
			tab.pick(tab.control('Display'), 'Display 1 · 1920 × 1080');
			session.picture();
			await tab.when(() => tab.control('Display').getAttribute('aria-label') === 'Display: Display 1');
		});
	});
	suite('sound', () => {
		test('muting from the bar shows and reaches the sound', async () => {
			const { tab, session } = await live();
			const sound = tab.control('Mute Sound');
			// The sound is read with the picture's details, once a second.
			await tab.when(() => session.soundOperations.length > 0);
			click(sound);
			await tab.when(() => sound.getAttribute('aria-pressed') === 'true' && sound.getAttribute('aria-label') === 'Unmute Sound');
			assert.ok(session.soundOperations.some(operation => operation.type === 'muted' && operation.muted));
		});
		test('sound that stops says why', async () => {
			const { tab, session } = await live();
			const sound = tab.control('Mute Sound');
			session.sound = { phase: 'unavailable', enabled: true, muted: false, playbackFailed: false, error: { code: 'Unauthorized', message: 'not shared' } };
			await tab.when(() => sound.dataset.sound === 'unavailable');
			assert.ok(sound.getAttribute('aria-label')?.includes('doesn’t share its sound'), String(sound.getAttribute('aria-label')));
		});
		test('the button of stopped sound asks for it again and brings it back', async () => {
			const { tab, session } = await live();
			const sound = tab.control('Mute Sound');
			session.sound = { phase: 'unavailable', enabled: true, muted: false, playbackFailed: false, error: { code: 'Unauthorized', message: 'not shared' } };
			await tab.when(() => sound.dataset.sound === 'unavailable');
			click(sound);
			await tab.when(() => sound.dataset.sound === undefined && sound.getAttribute('aria-label') === 'Mute Sound');
			assert.strictEqual(session.soundOperations.at(-1)?.type, 'enabled');
		});
	});
	suite('privacy mode', () => {
		test('privacy mode shows off before it is asked for', async () => {
			const { tab } = await live();
			assert.strictEqual(tab.control('Privacy').getAttribute('aria-pressed'), 'false');
		});
		test('privacy mode turns on and is said', async () => {
			const { tab } = await live();
			const privacy = tab.control('Privacy');
			click(privacy);
			await tab.when(() => privacy.getAttribute('aria-pressed') === 'true');
			assert.deepStrictEqual(fixture.systemActions.privacyRequests, [true]);
			assert.ok(tab.text('.desktop-stats-notice').startsWith('Privacy mode on'), tab.text('.desktop-stats-notice'));
		});
		test('a refused change is said and leaves privacy mode as it was', async () => {
			const { tab } = await live();
			const privacy = tab.control('Privacy');
			click(privacy);
			await tab.when(() => privacy.getAttribute('aria-pressed') === 'true');
			fixture.systemActions.privacyError = 'Not allowed there';
			click(privacy);
			await tab.when(() => tab.text('.desktop-stats-notice') === 'Privacy mode: Not allowed there');
			assert.strictEqual(privacy.getAttribute('aria-pressed'), 'true');
		});
		test('closing the tab turns privacy mode off there', async () => {
			const { tab } = await live();
			const privacy = tab.control('Privacy');
			click(privacy);
			await tab.when(() => privacy.getAttribute('aria-pressed') === 'true');
			assert.ok(await fixture.tabService.close(tab.view.id));
			assert.strictEqual(fixture.systemActions.privacyRequests.at(-1), false);
		});
	});
	suite('more', () => {
		test('More offers recording, then the computer\'s actions and diagnostics, then this tab\'s', async () => {
			// Swapping Control and Command is offered between a Mac and this PC.
			const { tab } = await live();
			click(tab.control('More'));
			const swap = isMacintosh ? 'Swap Control and Command,' : '';
			assert.strictEqual(fixture.contextMenu.lastMenu().join(), `Record Screen,${swap}-,Lock Screen,Restart or Shut Down…,-,Export Diagnostics…,Open Logs Folder,-,Show Picture Details,Disconnect`);
		});
		(isMacintosh ? test : test.skip)('turning the exchange off is sent once the keys are let go, and remembered for the computer', async () => {
			const { tab, session } = await live();
			tab.pick(tab.control('More'), 'Swap Control and Command');
			assert.deepStrictEqual(session.sent.slice(-2), [{ type: 'releaseSessionInput' }, { type: 'swapSessionModifiers', enabled: false }]);
			await tab.when(() => fixture.uiState.values.get('desktop.swapModifiers.studio') === false);
			click(tab.control('More'));
			assert.strictEqual(fixture.contextMenu.menus.at(-1)?.actions.find(item => item.label === 'Swap Control and Command')?.checked, false);
		});

		/** The bar's button of the running recording, if one runs. */
		function recording(tab: DesktopTab): HTMLButtonElement | undefined {
			return [...tab.element.querySelectorAll<HTMLButtonElement>('.desktop-bar-recording')].find(button => !button.hidden);
		}
		test('a recording shows in the bar, its length advancing', async () => {
			const { tab } = await live();
			tab.pick(tab.control('More'), 'Record Screen');
			await tab.when(() => recording(tab)?.textContent === '0:00');
			const running = fixture.runtime.recordings.get('1');
			assert.ok(running);
			fixture.runtime.recordings.set('1', { ...running, durationMs: '5000' });
			await tab.when(() => recording(tab)?.textContent === '0:05');
		});
		test('stopping a recording from the bar says where it went, and stops and releases it', async () => {
			const { tab } = await live();
			tab.pick(tab.control('More'), 'Record Screen');
			await tab.when(() => recording(tab) !== undefined);
			click(recording(tab));
			await tab.when(() => recording(tab) === undefined && tab.text('.desktop-stats-notice') === 'Recording saved to the recordings folder.');
			assert.deepStrictEqual([fixture.runtime.stopped, fixture.runtime.released], [['1'], ['1']]);
		});
	});
	suite('ends', () => {
		test('a dropped connection says it reconnects, over the last picture dimmed', async () => {
			const { tab, session } = await live();
			session.end('linkLost');
			await tab.when(() => !tab.overlay.hidden && tab.overlay.textContent.includes('Reconnecting to Studio PC'));
			assert.ok(tab.stage.classList.contains('desktop-dimmed'));
		});
		test('a dropped connection is tried again by itself', async () => {
			const { tab, session } = await live();
			session.end('linkLost');
			await fixture.opened(2);
			fixture.sessions[1].picture();
			await tab.when(() => tab.overlay.hidden === true);
		});
		test('a failed encoder there is said, and the tab reconnects at once', async () => {
			const { tab, session } = await live();
			session.end('hostEncoderFailed');
			await tab.when(() => !tab.overlay.hidden && tab.overlay.textContent.includes('Studio PC couldn’t keep encoding its screen.'));
			await fixture.opened(2);
			fixture.sessions[1].picture();
			await tab.when(() => tab.overlay.hidden === true);
		});

		/** A tab whose screen was ended because that computer withdrew its permission. */
		async function withdrawn(): Promise<DesktopTab> {
			const { tab, session } = await live();
			session.end('hostPermission');
			await tab.when(() => tab.overlay.textContent.includes('Studio PC no longer allows VniLink to record its screen.'));
			return tab;
		}
		test('a withdrawn permission says what someone there must do', async () => {
			const tab = await withdrawn();
			assert.ok(tab.overlay.textContent.includes('system settings'), tab.overlay.textContent);
		});
		test('an end is announced as an alert', async () => {
			const tab = await withdrawn();
			assert.strictEqual(tab.overlay.getAttribute('role'), 'alert');
		});
		test('the picture\'s conditions go with it', async () => {
			const tab = await withdrawn();
			assert.strictEqual(tab.required('.desktop-stats-condition').hidden, true);
		});
		test('a withdrawn permission is not tried again by itself', async () => {
			await withdrawn();
			await animationFrames(1);
			assert.strictEqual(fixture.sessions.length, 1);
		});
		test('Try Again after a withdrawn permission opens the screen', async () => {
			const tab = await withdrawn();
			click(tab.overlay.querySelector<HTMLButtonElement>('button.primary'));
			await fixture.opened(2);
			fixture.sessions[1].picture();
			await tab.when(() => tab.overlay.hidden === true);
		});

		/** A tab whose reconnect that computer refused. */
		async function refused(): Promise<DesktopTab> {
			const { tab, session } = await live();
			fixture.remoteDesktop.failures.push('turnedOff');
			session.end('linkLost');
			await tab.when(() => tab.overlay.textContent.includes('Studio PC doesn’t share its screen with this computer.'));
			return tab;
		}
		test('a refusal offers to try again or close the tab', async () => {
			const tab = await refused();
			assert.deepStrictEqual([...tab.overlay.querySelectorAll('button')].map(button => button.textContent), ['Try Again', 'Close Tab']);
		});
		test('a refused tab says it is not connected', async () => {
			const tab = await refused();
			assert.strictEqual(tab.role(), 'Not connected');
		});
		test('Try Again after a refusal opens the screen', async () => {
			const tab = await refused();
			click(tab.overlay.querySelector<HTMLButtonElement>('button.primary'));
			await fixture.opened(2);
		});
		test('closing the tab ends its session', async () => {
			const { tab, session } = await live();
			assert.ok(await fixture.tabService.close(tab.view.id));
			assert.ok(session.disposed);
		});
	});
	suite('viewing only', () => {
		test('View Screen opens a view-only tab', () => {
			assert.strictEqual(fixture.open(false).control, false);
		});
		test('a view-only tab says it is viewing', async () => {
			const { tab } = await live(false);
			assert.strictEqual(tab.role(), 'Viewing');
		});
		test('a view-only tab offers to switch nothing there: no display, no privacy mode', async () => {
			const { tab } = await live(false);
			const controls = tab.controls();
			assert.ok(!controls.some(label => label.startsWith('Display') || label.startsWith('Privacy')), controls.join());
		});
		test('a view-only tab sends no input', async () => {
			const { tab, session } = await live(false);
			tab.press(tab.canvas, 'keydown', 'KeyA', 'a');
			assert.deepStrictEqual(session.sent, []);
		});
		test('viewing, the other pointer is drawn under this one, which keeps its own shape', async () => {
			const { tab, session } = await live(false);
			tab.canvas.dispatchEvent(new MouseEvent('mouseenter'));
			session.pointer(100, 100);
			assert.strictEqual(tab.mark.hidden, false);
			assert.strictEqual(tab.canvas.style.cursor, '');
		});
		test('a picture this computer cannot show reconnects once, as the next offer leaves its codec out', async () => {
			const { session } = await live(false);
			session.end('cantShow');
			await fixture.opened(2);
		});
		test('a picture this computer still cannot show is said, and not tried again', async () => {
			const { tab, session } = await live(false);
			session.end('cantShow');
			await fixture.opened(2);
			fixture.sessions[1].end('cantShow');
			await tab.when(() => tab.overlay.textContent.includes('This computer can’t show the screen of Studio PC.'));
			assert.strictEqual(fixture.sessions.length, 2);
		});
	});
});
