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

/** Stands in for @vnilink/elg's owner, recording what the window asks of it. */
class TestGlassOwner implements LiquidGlassOwner {
	public readonly calls: (readonly [string, ...unknown[]])[] = [];

	constructor(private readonly supported = true, private readonly view = 4) { }

	public isGlassSupported(): boolean {
		return this.supported;
	}

	public addView(view: Buffer, options?: GlassOptions): number {
		this.calls.push(['add', view === handle, options]);
		return this.view;
	}

	public removeView(id: number): boolean {
		this.calls.push(['remove', id]);
		return true;
	}

	public dispose(): void {
		this.calls.push(['dispose']);
	}
}

const lightGlass = ['add', true, { opaque: true, tintColor: '#F1F1F3CC' }];

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
			assert.deepStrictEqual(window.calls.slice(-2), [['material', 'none'], ['background', '#202124']]);
		});
		test('mica returns once transparency is no longer reduced', () => {
			const material = createMaterial('win32', '10.0.22631');
			theme.update({ prefersReducedTransparency: true });
			theme.update({ prefersReducedTransparency: false });
			assert.strictEqual(material.material, 'mica');
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
		test('macOS 26 attaches glass in the light sidebar colour over an opaque backing', () => {
			const material = createGlassMaterial();
			assert.strictEqual(material.material, 'glass');
			// Glass sits on a window-coloured backing in the theme's sidebar colour, whatever the wallpaper.
			assert.deepStrictEqual(glass.calls, [lightGlass]);
			assert.deepStrictEqual(window.calls.slice(-2), [['vibrancy', null], ['background', '#00000000']]);
		});
		test('reduced transparency removes the glass and makes the window opaque', () => {
			const material = createGlassMaterial();
			theme.update({ prefersReducedTransparency: true });
			assert.strictEqual(material.material, 'none');
			assert.deepStrictEqual(glass.calls.at(-1), ['remove', 4]);
			assert.deepStrictEqual(window.calls.at(-1), ['background', '#fafafa']);
		});
		test('glass attaches again once transparency is no longer reduced', () => {
			// The system does not restore glass by itself.
			const material = createGlassMaterial();
			theme.update({ prefersReducedTransparency: true });
			theme.update({ prefersReducedTransparency: false });
			assert.strictEqual(material.material, 'glass');
			assert.deepStrictEqual(glass.calls.at(-1), lightGlass);
		});
		test('a theme change re-tints the glass with that theme\'s sidebar colour', () => {
			createGlassMaterial();
			theme.update({ shouldUseDarkColors: true });
			assert.deepStrictEqual(glass.calls.at(-1), ['add', true, { opaque: true, tintColor: '#26272BB8' }]);
		});
		test('a window keeps one glass owner for its life', () => {
			// A second owner would let the first remove its glass when collected.
			createGlassMaterial();
			theme.update({ prefersReducedTransparency: true });
			theme.update({ prefersReducedTransparency: false });
			theme.update({ shouldUseDarkColors: true });
			assert.strictEqual(created, 1);
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
		test('macOS before 26 uses sidebar vibrancy', () => {
			const material = createMaterial('darwin', '24.6.0');
			assert.strictEqual(material.material, 'vibrancy');
			assert.deepStrictEqual(window.calls.slice(-2), [['vibrancy', 'sidebar'], ['background', '#00000000']]);
		});
		test('a refused native backdrop leaves the window opaque', () => {
			window.refuseVibrancy = true;
			const material = createMaterial('darwin', '24.6.0');
			assert.strictEqual(material.material, 'none');
			assert.deepStrictEqual(window.calls.at(-1), ['background', '#fafafa']);
		});
	});
});
