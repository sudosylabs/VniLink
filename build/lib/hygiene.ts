// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/**
 * Keeps the repository about the project rather than a contributor's computer
 * or account: no home directories other than the placeholders, no git
 * author's handle, and the product's identity only where `product.json` and
 * the contact documents hold it. It also keeps comments in the project's own
 * Rust and TypeScript sources about code: a comment names symbols and the
 * behaviour they implement, never a document, a skill or a plan, which can
 * stay behind when the code changes.
 */
import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { noticeKindOf, readCopies } from './license.ts';
import { isCopyrightNotice } from './licenseNotice.ts';
import { readProduct, type Product } from './product.ts';

const run = promisify(execFile);

/** Home directories fixtures and docs may name. `Shared` and `Public` are macOS's and Windows's shared homes. */
const placeholderHomes = new Set(['someone', 'Shared', 'Public']);

/** The contact documents, which spell out the publisher's address and repository for people. */
const identityFiles = new Set(['product.json', 'SECURITY.md', 'CODE_OF_CONDUCT.md', 'CONTRIBUTING.md', 'REUSE.toml']);

/** Third-party sources keep their own text. */
const excludedDirectories = ['vendor/'];

// `Users/<name>` in macOS, Windows and root-relative forms; `/home/<name>` only as a path's start,
// so `contrib/home/...` is not a home. A name starts with a letter or digit, so `Users/..` is none.
const homePatterns = [/(?<![\w.$-])Users[\\/]+([A-Za-z0-9][\w.-]*)/g, /(?<![\w.$/\\-])\/home\/+([A-Za-z0-9][\w.-]*)/g];

export interface HygieneRules {
	/** Lowercase words of git authors' names and email handles. */
	readonly handles: readonly string[];
	/** Lowercase strings that name the publisher. */
	readonly identity: readonly string[];
}

export interface HygieneFinding {
	readonly path: string;
	readonly line: number;
	readonly message: string;
}

function words(value: string): string[] {
	return value.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(word => word.length >= 4 && !/^\p{N}+$/u.test(word));
}

/** What names the publisher in `product`: its reverse-domain owner, publisher, repository owner and mail domain. */
export function productIdentity(product: Product): string[] {
	const owner = product.appId.split('.').slice(1, -1);
	const repository = new URL(product.repositoryUrl).pathname.split('/')[1] ?? '';
	const domain = product.supportEmail.split('@')[1] ?? '';
	return [...new Set([...owner, product.publisherName, repository, domain].map(value => value.toLowerCase()).filter(value => value.length > 0))];
}

/** The rules for a repository whose authors are `authors` (`name\0email` lines, as `git log` prints them). */
export function hygieneRules(product: Product, authors: readonly string[]): HygieneRules {
	const handles = new Set<string>();
	for (const author of authors) {
		const [name = '', email = ''] = author.split('\0');
		for (const word of [...words(name), ...words(email.split('@')[0] ?? '')]) {
			handles.add(word);
		}
	}
	return { handles: [...handles], identity: productIdentity(product) };
}

function escape(value: string): string {
	return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** What in `text`, the contents of `path` relative to the repository, names a person, their computer or the publisher. */
export function findHygieneViolations(path: string, text: string, rules: HygieneRules): HygieneFinding[] {
	const findings: HygieneFinding[] = [];
	const handle = rules.handles.length > 0 ? new RegExp(`(?<![\\p{L}\\p{N}])(${rules.handles.map(escape).join('|')})(?![\\p{L}\\p{N}])`, 'giu') : undefined;
	const identityAllowed = identityFiles.has(path);
	const lines = text.split('\n');
	for (const [index, content] of lines.entries()) {
		const report = (message: string) => findings.push({ path, line: index + 1, message });
		for (const pattern of homePatterns) {
			for (const match of content.matchAll(pattern)) {
				if (!placeholderHomes.has(match[1])) {
					report(`home directory "${match[0]}": use /Users/someone, C:\\Users\\someone or /home/someone`);
				}
			}
		}
		for (const match of handle ? content.matchAll(handle) : []) {
			report(`git author's handle "${match[1]}": describe the project, not a contributor`);
		}
		// A license notice names whoever holds the copyright, in every source file.
		if (!identityAllowed && !isCopyrightNotice(content)) {
			const lower = content.toLowerCase();
			const value = rules.identity.find(value => lower.includes(value));
			if (value !== undefined) {
				report(`product identity "${value}": read it from product.json`);
			}
		}
	}
	return findings;
}

/** The text of the comment on a source line: a whole-line comment, or one after code. */
function commentOf(line: string): string | undefined {
	const whole = /^\s*(?:\/\/[/!]?|\/\*+|\*(?=\s))\s?(.*)$/.exec(line);
	if (whole) {
		return whole[1];
	}
	return /\s\/\/[/!]?\s(.*)$/.exec(line)?.[1];
}

/** What a comment may not say, and what to write instead. */
const commentRules: readonly { readonly pattern: RegExp; readonly message: (match: string) => string }[] = [
	// `summary.md` is a file a measurement writes, not a document of the project.
	{ pattern: /(?<![\w-])(?!summary\.md\b)[\w./-]*\.md\b/, message: match => `comment cites a markdown document "${match}": name the code it describes instead` },
	{ pattern: /\b(?:README|CONTRIBUTING)\b/, message: match => `comment cites a project document "${match}": name the code it describes instead` },
	{ pattern: /\.agents\/|\bdocs\/[\w.-]/, message: match => `comment cites a skill or documentation folder "${match}": name the code it describes instead` },
	{ pattern: /\b(?:step|phase|slice|milestone)s? \d+\b/i, message: match => `comment cites a plan "${match}": name the code it describes instead` },
	// A measurement keeps its hardware class, not one contributor's computer.
	{ pattern: /\b(?:this|my) (?:laptop|desktop computer)\b|\b(?:took|ran|measured) .{0,30}\bhere \(\d{4}-\d{2}-\d{2}\)/i, message: match => `comment describes one contributor's computer ("${match}"): state the hardware class and where the evidence lives` },
	{ pattern: /\b(?:RTX|GTX) ?\d{3,4}\b|\bRadeon RX ?\d{3,4}\b|\bApple M\d\b/, message: match => `comment names one GPU or chip model "${match}": state the hardware class` },
];

/** What in the comments of `text`, the contents of the source file `path`, points at a document or a plan, or at one contributor's computer. */
export function findCommentViolations(path: string, text: string): HygieneFinding[] {
	const findings: HygieneFinding[] = [];
	for (const [index, line] of text.split('\n').entries()) {
		const comment = commentOf(line);
		if (comment === undefined) {
			continue;
		}
		for (const { pattern, message } of commentRules) {
			const match = pattern.exec(comment);
			if (match) {
				findings.push({ path, line: index + 1, message: message(match[0]) });
			}
		}
	}
	return findings;
}

// A git hook's GIT_DIR and GIT_INDEX_FILE name the repository committing, not necessarily `root`'s.
async function git(root: string, args: readonly string[]): Promise<string> {
	const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('GIT_')));
	return (await run('git', ['-C', root, ...args], { env, maxBuffer: 64 * 1024 * 1024 })).stdout;
}

/** Checks every tracked or new file, outside ignored and third-party ones, and fails listing each finding. */
export async function checkHygiene(root: string): Promise<void> {
	const product = await readProduct(root);
	const authors = (await git(root, ['log', '--format=%an%x00%ae'])).split('\n').filter(line => line.length > 0);
	const rules = hygieneRules(product, authors);
	const copies = await readCopies(root);
	const paths = (await git(root, ['ls-files', '-z', '--cached', '--others', '--exclude-standard'])).split('\0')
		.filter(path => path.length > 0 && !excludedDirectories.some(directory => path.startsWith(directory)));
	const findings: HygieneFinding[] = [];
	for (const path of new Set(paths)) {
		let contents: Buffer;
		try {
			contents = await readFile(join(root, path));
		} catch (error) {
			// Deleted in the working tree but still in the index.
			if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
				continue;
			}
			throw error;
		}
		if (!contents.includes(0)) {
			const text = contents.toString('utf8');
			findings.push(...findHygieneViolations(path, text, rules));
			// Upstream copies keep their own comments.
			if (/\.(?:rs|ts|mjs)$/.test(path) && noticeKindOf(path, text, copies) === 'original') {
				findings.push(...findCommentViolations(path, text));
			}
		}
	}
	if (findings.length > 0) {
		throw new Error(`Personal details or document citations in the repository; see "Writing about the project" in the error-doc-voice skill:\n${findings.map(finding => `  ${finding.path}:${String(finding.line)}: ${finding.message}`).join('\n')}`);
	}
	console.log(`Hygiene: ${String(paths.length)} files name no home directory, author handle or product identity outside product.json and the contact documents, and no source comment cites a document, a plan or one contributor's computer.`);
}
