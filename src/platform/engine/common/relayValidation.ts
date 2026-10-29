import * as nls from '../../../nls.js';
import type { RelayInfo, RelayInput } from './engine.js';

export interface RelayValidationIssue {
	readonly index: number;
	readonly field: 'url' | 'token';
	readonly message: string;
}
/** Early feedback only; Rust remains authoritative about relay validity. */
export function validateRelays(relays: readonly RelayInput[], savedAuthenticatedUrls: ReadonlySet<string>): readonly RelayValidationIssue[] {
	if (relays.length < 1 || relays.length > 8) {
		return [{ index: -1, field: 'url', message: nls.localize('connection.relayCount', 'Enter between one and eight custom relays.') }];
	}
	const issues: RelayValidationIssue[] = [];
	const seen = new Set<string>();
	for (const [index, relay] of relays.entries()) {
		let valid = false;
		try {
			const url = new URL(relay.url);
			const host = url.hostname.replace(/^\[|\]$/g, '');
			const loopback = host === 'localhost' || host === '::1' || /^127(?:\.[0-9]{1,3}){3}$/.test(host);
			valid = new TextEncoder().encode(relay.url).length <= 512 && !/[\s\p{Cc}]/u.test(relay.url)
				&& (url.protocol === 'https:' || (url.protocol === 'http:' && loopback)) && !!url.hostname
				&& !url.username && !url.password && !url.search && !url.hash && !relay.url.includes('?') && !relay.url.includes('#') && url.port !== '0';
			if (seen.has(url.href)) {
				valid = false;
			}
			seen.add(url.href);
		} catch {
			valid = false;
		}
		if (!valid) {
			issues.push({ index, field: 'url', message: nls.localize('connection.invalidRelay', 'Enter a unique HTTPS URL without credentials, a query or a fragment. HTTP is allowed only on loopback.') });
		}
		if (relay.authenticated && ((!relay.replacementToken && !savedAuthenticatedUrls.has(relay.url))
			|| new TextEncoder().encode(relay.replacementToken).length > 256 || /[\s\p{Cc}]/u.test(relay.replacementToken))) {
			issues.push({ index, field: 'token', message: nls.localize('connection.invalidToken', 'Enter a token of at most 256 bytes without whitespace, or leave it blank to keep the saved token for this URL.') });
		}
	}
	return issues;
}

function canonicalUrl(url: string): string {
	try {
		return new URL(url.trim()).toString();
	} catch {
		return url.trim();
	}
}

/** Whether `draft` would change `saved`; a replacement credential is always a change. */
export function relaysChanged(saved: readonly RelayInfo[], draft: readonly RelayInput[]): boolean {
	const entries = (relays: readonly RelayInfo[]): string => JSON.stringify(relays.map(relay => `${canonicalUrl(relay.url)}|${String(relay.authenticated)}`).sort());
	return draft.some(relay => relay.replacementToken.length > 0) || entries(saved) !== entries(draft);
}
