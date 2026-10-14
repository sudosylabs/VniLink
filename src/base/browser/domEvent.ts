import { toDisposable, type IDisposable } from '../common/lifecycle.js';

export function addDisposableListener(element: HTMLElement, type: keyof HTMLElementEventMap, listener: (event: Event) => void): IDisposable {
	element.addEventListener(type, listener);
	return toDisposable(() => {
		element.removeEventListener(type, listener);
	});
}
