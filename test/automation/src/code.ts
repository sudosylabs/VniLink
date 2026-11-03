/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See LICENSE.vscode.txt in the project root for license information.
 *  Adapted from VS Code 1.141.0 test/automation/src/code.ts.
 *--------------------------------------------------------------------------------------------*/
// ---------------------------------------------------------------------------------------------
// Modifications Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Modifications licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only AND MIT
// ---------------------------------------------------------------------------------------------

import type { IElement, IElementLayout } from '../../../src/workbench/services/driver/windowDriver.js';
import type { WindowDriver } from './driver.js';

/**
 * The application under test, as the page objects reach it: the window
 * driver's calls, each retried until it answers as expected or a bound
 * passes.
 */
export class Code {

	constructor(public readonly driver: WindowDriver) { }

	public async getElement(selector: string): Promise<IElement | undefined> {
		return (await this.driver.renderer.getElements(selector, false)).at(0);
	}

	public async getElements(selector: string, recursive: boolean): Promise<IElement[] | undefined> {
		return this.driver.renderer.getElements(selector, recursive);
	}

	public async waitForTextContent(selector: string, textContent?: string, accept?: (result: string) => boolean, retryCount?: number): Promise<string> {
		const accepts = accept ?? ((result: string) => textContent !== undefined ? textContent === result : !!result);
		return await this.poll(
			async () => {
				const els = await this.driver.renderer.getElements(selector, false);
				if (els.length === 0) {
					throw new Error('Element not found for textContent');
				}
				return els[0].textContent;
			},
			s => accepts(s),
			`get text content '${selector}'`,
			retryCount
		);
	}

	public async waitAndClick(selector: string, retryCount = 200): Promise<void> {
		await this.poll(() => this.driver.renderer.click(selector), () => true, `click '${selector}'`, retryCount);
	}

	public async waitForSetValue(selector: string, value: string): Promise<void> {
		await this.poll(() => this.driver.renderer.setValue(selector, value), () => true, `set value '${selector}'`);
	}

	public async waitForElements(
		selector: string,
		recursive: boolean,
		accept: (result: IElement[]) => boolean = result => result.length > 0,
		retryCount?: number,
	): Promise<IElement[]> {
		return await this.poll(() => this.driver.renderer.getElements(selector, recursive), accept, `get elements '${selector}'`, retryCount);
	}

	public async waitForElement(selector: string, accept: (result: IElement | undefined) => boolean = result => !!result, retryCount = 200): Promise<IElement> {
		const element = await this.poll(() => this.driver.renderer.getElements(selector, false).then(els => els.at(0)), accept, `get element '${selector}'`, retryCount);
		if (!element) {
			throw new Error(`Element not found: ${selector}`);
		}
		return element;
	}

	public async waitForActiveElement(selector: string, retryCount = 200): Promise<void> {
		await this.poll(() => this.driver.renderer.isActiveElement(selector), r => r, `is active element '${selector}'`, retryCount);
	}

	/** The layout of `selector` once `accept` takes it: a style or size the page settles into. */
	public async waitForLayout(
		selector: string,
		styles: readonly string[],
		accept: (layout: IElementLayout) => boolean,
		retryCount = 50,
	): Promise<IElementLayout> {
		return await this.poll(() => this.driver.renderer.getLayout(selector, styles), accept, `get layout '${selector}'`, retryCount);
	}

	public async whenWorkbenchRestored(): Promise<void> {
		await this.poll(() => this.driver.renderer.whenWorkbenchRestored(), () => true, `when workbench restored`);
	}

	public wait(millis: number): Promise<void> {
		return new Promise(resolve => setTimeout(resolve, millis));
	}

	private async poll<T>(
		fn: () => Promise<T>,
		acceptFn: (result: T) => boolean,
		timeoutMessage: string,
		retryCount = 200,
		retryInterval = 100 // millis
	): Promise<T> {
		let trial = 1;
		let lastError = '';
		for (; ;) {
			if (trial > retryCount) {
				throw new Error(`Timeout: ${timeoutMessage} after ${String((retryCount * retryInterval) / 1000)} seconds.${lastError ? ` ${lastError}` : ''}`);
			}
			try {
				const result = await fn();
				if (acceptFn(result)) {
					return result;
				} else {
					lastError = 'Did not pass accept function';
				}
			} catch (e: unknown) {
				lastError = e instanceof Error ? e.message : String(e);
			}
			await this.wait(retryInterval);
			trial++;
		}
	}
}
