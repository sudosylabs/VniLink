/**
 * The text of the per-file license notices and the comment syntax each source
 * language carries them in; `license.ts` applies and checks them, and
 * `.agents/skills/licensing/SKILL.md` says which file gets which.
 */

/** How a language writes a comment that holds a notice. */
export type CommentStyle = 'slash' | 'css' | 'hash' | 'rem';

/** Languages whose files carry a notice, by extension. Data, markup and binary files are covered by `REUSE.toml`. */
const commentStyles: Readonly<Record<string, CommentStyle>> = {
	ts: 'slash',
	mts: 'slash',
	mjs: 'slash',
	js: 'slash',
	rs: 'slash',
	c: 'slash',
	cpp: 'slash',
	h: 'slash',
	css: 'css',
	sh: 'hash',
	ps1: 'hash',
	bat: 'rem',
};

/** The comment style of `path`'s language, or `undefined` for a file that cannot carry a notice. */
export function commentStyleOf(path: string): CommentStyle | undefined {
	const extension = path.slice(path.lastIndexOf('.') + 1);
	return Object.hasOwn(commentStyles, extension) ? commentStyles[extension] : undefined;
}

/** The year of the copyright line in every notice. */
export const noticeYear = 2026;

const rule = '-'.repeat(93);

/** The notice of a file written for this project. */
export function originalNotice(holder: string): readonly string[] {
	return [
		`Copyright (c) ${String(noticeYear)} ${holder}. All rights reserved.`,
		'Licensed under the GNU Affero General Public License, version 3 only.',
		'See LICENSE in the project root for license information.',
		'SPDX-License-Identifier: AGPL-3.0-only',
	];
}

/** The notice added after the original author's, in a file copied from VS Code and changed here. */
export function modificationNotice(holder: string): readonly string[] {
	return [
		`Modifications Copyright (c) ${String(noticeYear)} ${holder}. All rights reserved.`,
		'Modifications licensed under the GNU Affero General Public License, version 3 only.',
		'See LICENSE in the project root for license information.',
		'SPDX-License-Identifier: AGPL-3.0-only AND MIT',
	];
}

/** `lines` as a comment block in `style`, ending with a newline. */
export function renderNotice(style: CommentStyle, lines: readonly string[]): string {
	if (style === 'css') {
		return `${[`/* ${rule}`, ...lines.map(line => ` * ${line}`), ` * ${rule} */`].join('\n')}\n`;
	}
	const prefix = style === 'slash' ? '//' : style === 'hash' ? '#' : 'REM';
	return `${[rule, ...lines, rule].map(line => `${prefix} ${line}`).join('\n')}\n`;
}

/**
 * VS Code's own notice, as its files carry it, with `adaptation` (what this project changed) after the license line.
 * A file copied from VS Code keeps this block; `modificationNotice` follows it.
 */
export function microsoftNotice(style: CommentStyle, adaptation: readonly string[] = []): string {
	const lines = [
		'Copyright (c) Microsoft Corporation. All rights reserved.',
		'Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.',
		...adaptation,
	];
	if (style === 'hash' || style === 'rem') {
		return renderNotice(style, lines);
	}
	if (style === 'css') {
		return `${[`/* ${rule}`, ...lines.map(line => ` *  ${line}`), ` *${rule.slice(0, -1)} */`].join('\n')}\n`;
	}
	return `${[`/*${rule}`, ...lines.map(line => ` *  ${line}`), ` *${rule.slice(0, -1)}*/`].join('\n')}\n`;
}

/** The leading comment that holds VS Code's notice, if `text` starts with one. */
export function leadingMicrosoftNotice(text: string, style: CommentStyle): string | undefined {
	let block: string | undefined;
	if (style === 'hash' || style === 'rem') {
		const closing = `${style === 'hash' ? '#' : 'REM'} ${rule}`;
		const lines = text.split('\n');
		const end = lines.indexOf(closing, 1);
		block = lines[0] === closing && end > 0 ? `${lines.slice(0, end + 1).join('\n')}\n` : undefined;
	} else if (text.startsWith('/*') && text.includes('*/\n')) {
		block = text.slice(0, text.indexOf('*/\n') + 3);
	}
	return block?.includes('Copyright (c) Microsoft Corporation') ? block : undefined;
}

/** Whether `line` is a copyright line of a notice, which names whoever holds the copyright. */
export function isCopyrightNotice(line: string): boolean {
	return /\bCopyright \(c\) \d{4} .+\. All rights reserved\.$/.test(line);
}
