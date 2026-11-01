// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

import { Disposable } from '../../../base/common/lifecycle.js';
import type { DesktopPicture, DesktopRegion } from '../../services/remoteDesktop/remoteDesktopService.js';

/**
 * `next` in place of `first`, which it replaces before it is drawn: a change
 * when either is, where either says it changed. A repeat changes nothing; a
 * still, or a change that cannot say, may have changed anything.
 */
function replacing(first: DesktopPicture, next: DesktopPicture): DesktopPicture {
	if (first.role !== 'changed' && next.role !== 'changed') {
		return next;
	}
	const where = (picture: DesktopPicture): readonly DesktopRegion[] | undefined => picture.role === 'unchanged' ? [] : picture.role === 'changed' ? picture.regions : undefined;
	const before = where(first);
	const after = where(next);
	return { ...next, role: 'changed', regions: before && after ? [...before, ...after] : undefined };
}

/**
 * Holds the newest decoded picture until the screen's next frame, then
 * draws it: pictures that arrive faster than the screen refreshes replace
 * the one waiting, so the shown picture is never behind the decoder. A
 * still waiting is not replaced by a repeat of the same screen, and a
 * picture that replaces another carries its changes too.
 */
export class NextFramePresenter extends Disposable {
	private pending: DesktopPicture | undefined;
	private handle: number | undefined;

	constructor(
		private readonly targetWindow: Window,
		private readonly present: (picture: DesktopPicture) => void,
		private readonly skip: (picture: DesktopPicture) => void,
	) {
		super();
	}

	public show(picture: DesktopPicture): void {
		if (picture.role === 'unchanged' && this.pending?.role === 'still') {
			picture.frame.close();
			return;
		}
		let next = picture;
		if (this.pending) {
			next = replacing(this.pending, picture);
			this.pending.frame.close();
			this.skip(this.pending);
		}
		this.pending = next;
		this.handle ??= this.targetWindow.requestAnimationFrame(() => {
			this.handle = undefined;
			const next = this.pending;
			this.pending = undefined;
			if (next) {
				this.present(next);
			}
		});
	}

	public override dispose(): void {
		if (this.handle !== undefined) {
			this.targetWindow.cancelAnimationFrame(this.handle);
			this.handle = undefined;
		}
		this.pending?.frame.close();
		this.pending = undefined;
		super.dispose();
	}
}
