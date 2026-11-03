/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { illegalArgument } from './errors.js';
import { matchesSomeScheme, Schemas } from './network.js';
import { isEqual } from './resources.js';
import { URI, UriComponents } from './uri.js';

export interface MarkdownStringTrustedOptions {
}

export interface IMarkdownString {
	readonly value: string;
	readonly isTrusted?: boolean | MarkdownStringTrustedOptions;
	readonly supportThemeIcons?: boolean;
	readonly supportHtml?: boolean;
	/** @internal */
	readonly supportAlertSyntax?: boolean;
	readonly baseUri?: UriComponents;
	uris?: { [href: string]: UriComponents };
}

export class MarkdownString implements IMarkdownString {

	public value: string;
	public isTrusted?: boolean | MarkdownStringTrustedOptions;
	public supportThemeIcons?: boolean;
	public supportHtml?: boolean;
	public supportAlertSyntax?: boolean;
	public baseUri?: URI;
	public uris?: { [href: string]: UriComponents } | undefined;

	constructor(
		value: string = '',
		isTrustedOrOptions: boolean | { isTrusted?: boolean | MarkdownStringTrustedOptions; supportThemeIcons?: boolean; supportHtml?: boolean; supportAlertSyntax?: boolean } = false,
	) {
		this.value = value;
		if (typeof this.value !== 'string') {
			throw illegalArgument('value');
		}

		if (typeof isTrustedOrOptions === 'boolean') {
			this.isTrusted = isTrustedOrOptions;
			this.supportThemeIcons = false;
			this.supportHtml = false;
			this.supportAlertSyntax = false;
		}
		else {
			this.isTrusted = isTrustedOrOptions.isTrusted ?? undefined;
			this.supportThemeIcons = isTrustedOrOptions.supportThemeIcons ?? false;
			this.supportHtml = isTrustedOrOptions.supportHtml ?? false;
			this.supportAlertSyntax = isTrustedOrOptions.supportAlertSyntax ?? false;
		}
	}
}

export function isMarkdownString(thing: unknown): thing is IMarkdownString {
	if (thing instanceof MarkdownString) {
		return true;
	} else if (thing && typeof thing === 'object') {
		return typeof (<IMarkdownString>thing).value === 'string'
			&& (typeof (<IMarkdownString>thing).isTrusted === 'boolean' || typeof (<IMarkdownString>thing).isTrusted === 'object' || (<IMarkdownString>thing).isTrusted === undefined)
			&& (typeof (<IMarkdownString>thing).supportThemeIcons === 'boolean' || (<IMarkdownString>thing).supportThemeIcons === undefined)
			&& (typeof (<IMarkdownString>thing).supportAlertSyntax === 'boolean' || (<IMarkdownString>thing).supportAlertSyntax === undefined);
	}
	return false;
}

export function markdownStringEqual(a: IMarkdownString, b: IMarkdownString): boolean {
	if (a === b) {
		return true;
	} else if (!a || !b) {
		return false;
	} else {
		return a.value === b.value
			&& a.isTrusted === b.isTrusted
			&& a.supportThemeIcons === b.supportThemeIcons
			&& a.supportHtml === b.supportHtml
			&& a.supportAlertSyntax === b.supportAlertSyntax
			&& (a.baseUri === b.baseUri || !!a.baseUri && !!b.baseUri && isEqual(URI.from(a.baseUri), URI.from(b.baseUri)));
	}
}

export function escapeDoubleQuotes(input: string) {
	return input.replace(/"/g, '&quot;');
}

export function removeMarkdownEscapes(text: string): string {
	if (!text) {
		return text;
	}
	return text.replace(/\\([\\`*_{}[\]()#+\-.!~])/g, '$1');
}

export function parseHrefAndDimensions(href: string): { href: string; dimensions: string[] } {
	const dimensions: string[] = [];
	const splitted = href.split('|').map(s => s.trim());
	href = splitted[0];
	const parameters = splitted[1];
	if (parameters) {
		const heightFromParams = /height=(\d+)/.exec(parameters);
		const widthFromParams = /width=(\d+)/.exec(parameters);
		const height = heightFromParams ? heightFromParams[1] : '';
		const width = widthFromParams ? widthFromParams[1] : '';
		const widthIsFinite = isFinite(parseInt(width));
		const heightIsFinite = isFinite(parseInt(height));
		if (widthIsFinite) {
			dimensions.push(`width="${width}"`);
		}
		if (heightIsFinite) {
			dimensions.push(`height="${height}"`);
		}
	}
	return { href, dimensions };
}

/**
 * VS Code addresses some content with placeholder `http` URLs whose authority is wrapped in
 * underscores, such as `http://_vscodecontentref_/0`. Underscores are illegal in hostnames, so
 * no real site collides with this shape.
 */
const placeholderAuthority = /^https?:\/\/_[^/?#]*_(?:[/?#]|$)/i;

/** Whether a target still resolves once content is copied into another application. */
export function isPortableLinkTarget(target: string): boolean {
	const trimmed = target.trim();
	return matchesSomeScheme(trimmed, Schemas.http, Schemas.https, Schemas.mailto)
		&& !placeholderAuthority.test(trimmed);
}
