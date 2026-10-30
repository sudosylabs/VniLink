// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/** What the notice window shows: who asked, what, and when it runs (milliseconds since the epoch). */
export interface NoticeContent {
	readonly action: 'restart' | 'shutdown';
	readonly name: string;
	readonly deadline: number;
}

export type NoticeAnswer = 'cancel' | 'now';

/** The notice window's channels; its preload exposes only these. */
export const NOTICE_SHOW_CHANNEL = 'vnilink:notice-show';

export const NOTICE_RESPOND_CHANNEL = 'vnilink:notice-respond';
/** The window's backdrop, sent at load and whenever it changes; see `WindowMaterial`. */
export const NOTICE_MATERIAL_CHANNEL = 'vnilink:notice-material';

/** The page the notice window loads. */
export const NOTICE_URL = 'vnilink://app/notice.html';

export function isNoticeContent(value: unknown): value is NoticeContent {
	return typeof value === 'object' && value !== null && Object.keys(value).length === 3
		&& (Reflect.get(value, 'action') === 'restart' || Reflect.get(value, 'action') === 'shutdown')
		&& typeof Reflect.get(value, 'name') === 'string' && String(Reflect.get(value, 'name')).length <= 256
		&& Number.isSafeInteger(Reflect.get(value, 'deadline'));
}

export function isNoticeAnswer(value: unknown): value is NoticeAnswer {
	return value === 'cancel' || value === 'now';
}
