// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import assert from 'assert';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { ensureNoDisposablesAreLeakedInTestSuite } from '../../../../base/test/common/utils.js';
import { getWindowColors, type WindowColors } from '../../common/windowColors.js';

/** The role in `colors.css` each main-process colour copies. */
const roles: Record<keyof WindowColors, string> = {
	background: '--canvas',
	raisedBackground: '--surface-raised',
	glassTint: '--glass-tint',
	titlebar: '--opaque-chrome',
	titlebarForeground: '--text',
};

/** The custom properties a block of declarations sets, by name. */
function declarations(block: string): Map<string, string> {
	return new Map([...block.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map(([, name, value]) => [name, value.trim()]));
}

/** The body of the at-rule that opens with `header`, braces balanced. */
function atRule(css: string, header: string): string {
	const start = css.indexOf(`${header} {`);
	assert.ok(start >= 0, `colors.css has ${header}`);
	let depth = 0;
	for (let index = css.indexOf('{', start); index < css.length; index++) {
		depth += css[index] === '{' ? 1 : css[index] === '}' ? -1 : 0;
		if (depth === 0) {
			return css.slice(css.indexOf('{', start) + 1, index);
		}
	}
	throw new Error(`${header} is not closed`);
}

/** What `platform`'s root sets inside `block`. */
function platformRoles(block: string, platform: string): Map<string, string> {
	const match = new RegExp(`:root\\[data-platform='${platform}'\\] \\{([^}]*)\\}`).exec(block);
	return match ? declarations(match[1]) : new Map<string, string>();
}

/** A literal colour as lowercase `#rrggbb`, or `#rrggbbaa` when it is not opaque; `transparent` is no colour. */
function normalize(color: string): string | undefined {
	if (color === 'transparent') {
		return undefined;
	}
	const rgb = /^rgb\((\d+) (\d+) (\d+) \/ (\d+)%\)$/.exec(color);
	if (rgb) {
		const hex = (value: number): string => value.toString(16).padStart(2, '0');
		return `#${[rgb[1], rgb[2], rgb[3]].map(value => hex(Number(value))).join('')}${hex(Math.round(Number(rgb[4]) / 100 * 255))}`;
	}
	const short = /^#([\da-f])([\da-f])([\da-f])$/i.exec(color);
	if (short) {
		return `#${short.slice(1).map(digit => digit + digit).join('')}`.toLowerCase();
	}
	if (/^#[\da-f]{6}([\da-f]{2})?$/i.test(color)) {
		return color.toLowerCase().replace(/^(#[\da-f]{6})ff$/, '$1');
	}
	throw new Error(`Not a literal colour: ${color}`);
}

suite('windowColors', () => {
	ensureNoDisposablesAreLeakedInTestSuite();
	/** The palette each scheme resolves to on each platform. */
	const palettes = new Map<string, Map<string, string>>();
	suiteSetup(async () => {
		const css = await readFile(join(process.cwd(), 'src/platform/theme/browser/media/colors.css'), 'utf8');
		const root = /^:root \{([^}]*)\}/m.exec(css);
		assert.ok(root, 'colors.css starts its palette with a :root block');
		const light = declarations(root[1]);
		const dark = new Map(light);
		for (const [, block] of css.matchAll(/@media \(prefers-color-scheme: dark\) \{\s*:root \{([^}]*)\}/g)) {
			for (const [name, value] of declarations(block)) {
				dark.set(name, value);
			}
		}
		const lightChrome = atRule(css, '@media (forced-colors: none)');
		const darkChrome = atRule(css, '@media (forced-colors: none) and (prefers-color-scheme: dark)');
		for (const platform of ['darwin', 'win32', 'linux']) {
			palettes.set(`${platform} light`, new Map([...light, ...platformRoles(lightChrome, platform)]));
			palettes.set(`${platform} dark`, new Map([...dark, ...platformRoles(lightChrome, platform), ...platformRoles(darkChrome, platform)]));
		}
	});
	test('each colour the main process paints is its role in colors.css, on each platform', () => {
		for (const [name, palette] of palettes) {
			const [platform, scheme] = name.split(' ');
			const colors = getWindowColors(scheme === 'dark', platform);
			for (const [field, role] of Object.entries(roles) as [keyof WindowColors, string][]) {
				const value = palette.get(role);
				assert.ok(value, `colors.css sets ${role}`);
				assert.strictEqual(colors[field], normalize(value), `${name} ${field} is ${role}`);
			}
		}
	});
	test('pages are the same on every platform; only the chrome is the platform\'s own', () => {
		for (const scheme of [false, true]) {
			const [darwin, ...others] = ['darwin', 'win32', 'linux'].map(platform => getWindowColors(scheme, platform));
			for (const colors of others) {
				assert.deepStrictEqual({ ...colors, titlebar: '' }, { ...darwin, titlebar: '' });
				assert.notStrictEqual(colors.titlebar, darwin.titlebar);
			}
		}
	});
});
