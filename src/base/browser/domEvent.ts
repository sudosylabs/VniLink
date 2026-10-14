import { toDisposable, type IDisposable } from '../common/lifecycle.js';

export function addDisposableListener(element: EventTarget, type: keyof HTMLElementEventMap | keyof WindowEventMap, listener: (event: Event) => void): IDisposable {
	element.addEventListener(type, listener);
	return toDisposable(() => {
		element.removeEventListener(type, listener);
	});
}
