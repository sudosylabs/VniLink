// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { EventEmitter } from 'events';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import type { GlassOptions, LiquidGlassOwner } from '../../electron-main/liquidGlass.js';
import { WindowMaterial } from '../../electron-main/windowMaterial.js';

type MaterialWindow = ConstructorParameters<typeof WindowMaterial>[0];

type MaterialTheme = NonNullable<ConstructorParameters<typeof WindowMaterial>[4]>;

const handle = Buffer.alloc(8, 1);

/** A window that records what its backdrop asks of it. */
class TestMaterialWindow implements MaterialWindow {
	public readonly calls: (readonly [string, unknown])[] = [];
	/** Whether the system refuses vibrancy, as a supported OS still can. */
	public refuseVibrancy = false;

	public isDestroyed(): boolean {
		return false;
	}

	public getNativeWindowHandle(): Buffer {
		return handle;
	}

	public setBackgroundMaterial(material: 'auto' | 'none' | 'mica' | 'acrylic' | 'tabbed'): void {
		this.calls.push(['material', material]);
	}

	public setVibrancy(type: Parameters<MaterialWindow['setVibrancy']>[0]): void {
		if (this.refuseVibrancy) {
			throw new Error('Native composition unavailable');
		}
		this.calls.push(['vibrancy', type]);
	}

	public setBackgroundColor(color: string): void {
		this.calls.push(['background', color]);
	}
}

/** Electron's `nativeTheme` as the test sets it; `update` changes it and emits `updated`. */
class TestNativeTheme extends EventEmitter implements MaterialTheme {
	public shouldUseDarkColors = false;
	public prefersReducedTransparency = false;
	public shouldUseHighContrastColors = false;

	public update(changes: Partial<Pick<TestNativeTheme, 'shouldUseDarkColors' | 'prefersReducedTransparency'>>): void {
		Object.assign(this, changes);
		this.emit('updated');
	}
}

/**
 * Stands in for @vnilink/elg's owner, recording what the window asks of it.
 * As in elg, the whole-window view is one per window, so adding it again
 * updates it, and each framed view is a new one.
 */
class TestGlassOwner implements LiquidGlassOwner {
	public readonly calls: (readonly [string, ...unknown[]])[] = [];
	private readonly views = new Map<number, boolean>();
	private next: number;

	constructor(private readonly supported = true, private readonly view = 4) {
		this.next = view;
	}

	public isGlassSupported(): boolean {
		return this.supported;
	}

	public addView(view: Buffer, options?: GlassOptions): number {
		this.calls.push(['add', view === handle, options]);
		if (this.view < 0) {
			return this.view;
		}
		const framed = options?.frame !== undefined;
		const existing = framed ? undefined : [...this.views].find(([, isFramed]) => !isFramed)?.[0];
		if (existing !== undefined) {
			return existing;
		}
		const id = this.next++;
		this.views.set(id, framed);
		return id;
	}

	public updateView(id: number, options?: GlassOptions): boolean {
		this.calls.push(['update', id, options]);
		return this.views.get(id) === (options?.frame !== undefined);
	}

	public removeView(id: number): boolean {
		this.calls.push(['remove', id]);
		return this.views.delete(id);
	}

	public dispose(): void {
		this.calls.push(['dispose']);
	}
}

/** The light theme's glass tint; dark glass has none. */
const LIGHT_TINT = '#e8e8ecb3';

const windowGlass = ['add', true, { opaque: true, tintColor: LIGHT_TINT }];

const darkWindowGlass = ['add', true, { opaque: true, tintColor: undefined }];

const panel = { x: 8, y: 36, width: 224, height: 600, cornerRadius: 10 };

/** The framed glass the window puts under `panel`, in `tintColor`; pass `null` for dark, untinted glass. */
function panelGlass(bounds: typeof panel, tintColor: string | null = LIGHT_TINT): GlassOptions {
	return {
		opaque: true,
		tintColor: tintColor ?? undefined,
		cornerRadius: bounds.cornerRadius,
		frame: { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height, anchors: { top: true, left: true, bottom: true } },
	};
}

suite('WindowMaterial', () => {
	const disposables = ensureNoDisposablesAreLeakedInTestSuite();
	let window: TestMaterialWindow;
	let theme: TestNativeTheme;
	setup(() => {
		window = new TestMaterialWindow();
		theme = new TestNativeTheme();
	});
	function createMaterial(platform: string, release: string, createGlass: () => LiquidGlassOwner | undefined = () => undefined): WindowMaterial {
		return disposables.add(new WindowMaterial(window, platform, release, createGlass, theme));
	}
	suite('Mica', () => {
		test('Windows 11 shows mica behind a transparent background', () => {
			const material = createMaterial('win32', '10.0.22631');
			assert.strictEqual(material.material, 'mica');
			assert.deepStrictEqual(window.calls.slice(-2), [['material', 'mica'], ['background', '#00000000']]);
		});
		test('reduced transparency turns mica off and paints the dark theme colour', () => {
			const material = createMaterial('win32', '10.0.22631');
			theme.update({ prefersReducedTransparency: true, shouldUseDarkColors: true });
			assert.strictEqual(material.material, 'none');
			assert.deepStrictEqual(window.calls.slice(-2), [['material', 'none'], ['background', '#212123']]);
		});
		test('mica returns once transparency is no longer reduced', () => {
			const material = createMaterial('win32', '10.0.22631');
			theme.update({ prefersReducedTransparency: true });
			theme.update({ prefersReducedTransparency: false });
			assert.strictEqual(material.material, 'mica');
		});
		test('a panel shows mica through the page, with no glass to place', () => {
			const material = createMaterial('win32', '10.0.22631');
			const count = window.calls.length;
			material.setPanel(panel);
			assert.strictEqual(window.calls.length, count);
		});
		test('a disposed backdrop stops following the theme', () => {
			const material = createMaterial('win32', '10.0.22631');
			material.dispose();
			const count = window.calls.length;
			theme.update({ shouldUseDarkColors: true });
			assert.strictEqual(window.calls.length, count);
			assert.strictEqual(theme.listenerCount('updated'), 0);
		});
	});
	suite('Liquid Glass', () => {
		let glass: TestGlassOwner;
		let created: number;
		function createGlassMaterial(): WindowMaterial {
			return createMaterial('darwin', '25.0.0', () => {
				created++;
				return glass;
			});
		}
		setup(() => {
			glass = new TestGlassOwner();
			created = 0;
		});
		test('macOS 26 attaches glass over the window\'s content material, in the light theme\'s grey', () => {
			const material = createGlassMaterial();
			assert.strictEqual(material.material, 'glass');
			// Glass refracts the window's own backing, as AppKit's sidebar does, whatever the wallpaper.
			assert.deepStrictEqual(glass.calls, [windowGlass]);
			assert.deepStrictEqual(window.calls.slice(-2), [['vibrancy', null], ['background', '#00000000']]);
		});
		test('reduced transparency removes the glass and makes the window opaque', () => {
			const material = createGlassMaterial();
			theme.update({ prefersReducedTransparency: true });
			assert.strictEqual(material.material, 'none');
			assert.deepStrictEqual(glass.calls.at(-1), ['remove', 4]);
			assert.deepStrictEqual(window.calls.at(-1), ['background', '#ffffff']);
		});
		test('glass attaches again once transparency is no longer reduced', () => {
			// The system does not restore glass by itself.
			const material = createGlassMaterial();
			theme.update({ prefersReducedTransparency: true });
			theme.update({ prefersReducedTransparency: false });
			assert.strictEqual(material.material, 'glass');
			assert.deepStrictEqual(glass.calls.at(-1), windowGlass);
		});
		test('dark glass is untinted, since it reads apart from the page as it is', () => {
			createGlassMaterial();
			theme.update({ shouldUseDarkColors: true });
			assert.deepStrictEqual(glass.calls.at(-1), darkWindowGlass);
		});
		test('a window keeps one glass owner for its life', () => {
			// A second owner would let the first remove its glass when collected.
			createGlassMaterial();
			theme.update({ prefersReducedTransparency: true });
			theme.update({ prefersReducedTransparency: false });
			theme.update({ shouldUseDarkColors: true });
			assert.strictEqual(created, 1);
		});
		test('a panel gets glass of its own, framed above the window\'s and following its height', () => {
			const material = createGlassMaterial();
			material.setPanel(panel);
			assert.deepStrictEqual(glass.calls.at(-1), ['add', true, panelGlass(panel)]);
		});
		test('a panel that moves keeps its glass, given the new frame', () => {
			const material = createGlassMaterial();
			material.setPanel(panel);
			const moved = { ...panel, y: 8, height: 640 };
			material.setPanel(moved);
			assert.deepStrictEqual(glass.calls.at(-1), ['update', 5, panelGlass(moved)]);
		});
		test('a hidden panel removes its glass and leaves the window\'s', () => {
			const material = createGlassMaterial();
			material.setPanel(panel);
			material.setPanel(undefined);
			assert.deepStrictEqual(glass.calls.at(-1), ['remove', 5]);
			assert.strictEqual(material.material, 'glass');
		});
		test('reduced transparency removes the panel\'s glass with the window\'s, and both return in order', () => {
			const material = createGlassMaterial();
			material.setPanel(panel);
			theme.update({ prefersReducedTransparency: true });
			assert.deepStrictEqual(glass.calls.slice(-2), [['remove', 5], ['remove', 4]]);
			theme.update({ prefersReducedTransparency: false });
			// The window's glass first, so the panel's is added above it.
			assert.deepStrictEqual(glass.calls.slice(-2), [windowGlass, ['add', true, panelGlass(panel)]]);
		});
		test('a theme change re-tints the panel\'s glass where it is', () => {
			const material = createGlassMaterial();
			material.setPanel(panel);
			theme.update({ shouldUseDarkColors: true });
			assert.deepStrictEqual(glass.calls.slice(-2), [darkWindowGlass, ['update', 5, panelGlass(panel, null)]]);
		});
		test('disposing the backdrop releases the glass owner', () => {
			const material = createGlassMaterial();
			material.dispose();
			assert.deepStrictEqual(glass.calls.at(-1), ['dispose']);
		});
	});
	suite('vibrancy', () => {
		const unavailableGlass: readonly (readonly [string, () => LiquidGlassOwner | undefined])[] = [
			['not staged', () => undefined],
			['unsupported', () => new TestGlassOwner(false)],
			['refused', () => new TestGlassOwner(true, -1)],
		];
		for (const [reason, createGlass] of unavailableGlass) {
			test(`macOS 26 falls back to sidebar vibrancy when glass is ${reason}`, () => {
				const material = createMaterial('darwin', '25.0.0', createGlass);
				assert.strictEqual(material.material, 'vibrancy');
				assert.deepStrictEqual(window.calls.slice(-2), [['vibrancy', 'sidebar'], ['background', '#00000000']]);
			});
		}
		test('a panel shows vibrancy through the page, with no glass to place', () => {
			const glass = new TestGlassOwner(false);
			const material = createMaterial('darwin', '25.0.0', () => glass);
			material.setPanel(panel);
			assert.deepStrictEqual(glass.calls, []);
		});
		test('macOS before 26 uses sidebar vibrancy', () => {
			const material = createMaterial('darwin', '24.6.0');
			assert.strictEqual(material.material, 'vibrancy');
			assert.deepStrictEqual(window.calls.slice(-2), [['vibrancy', 'sidebar'], ['background', '#00000000']]);
		});
		test('a refused native backdrop leaves the window opaque', () => {
			window.refuseVibrancy = true;
			const material = createMaterial('darwin', '24.6.0');
			assert.strictEqual(material.material, 'none');
			assert.deepStrictEqual(window.calls.at(-1), ['background', '#ffffff']);
		});
	});
});
