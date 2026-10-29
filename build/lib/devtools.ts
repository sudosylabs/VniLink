/** Small owned DevTools connection used only by the external packaged-app verifier. */

interface PendingRequest {
	readonly resolve: (value: unknown) => void;
	readonly reject: (error: Error) => void;
	readonly timer: NodeJS.Timeout;
}

interface DevToolsMessage {
	readonly id?: number;
	readonly result?: unknown;
	readonly error?: { readonly message: string };
}

export class DevToolsConnection {
	#nextId = 0;
	readonly #pending = new Map<number, PendingRequest>();
	#socket: WebSocket | undefined;

	public async connect(url: string): Promise<void> {
		const socket = new WebSocket(url);
		this.#socket = socket;
		await new Promise<void>((resolve, reject) => {
			const timer = setTimeout(() => {
				reject(new Error('DevTools connection timed out'));
			}, 10000);
			socket.addEventListener('open', () => {
				clearTimeout(timer);
				resolve();
			}, { once: true });
			socket.addEventListener('error', () => {
				clearTimeout(timer);
				reject(new Error('DevTools connection failed'));
			}, { once: true });
		});
		socket.addEventListener('message', event => {
			const message = JSON.parse(String(event.data)) as DevToolsMessage;
			const request = message.id === undefined ? undefined : this.#pending.get(message.id);
			if (request && message.id !== undefined) {
				this.#pending.delete(message.id);
				clearTimeout(request.timer);
				if (message.error) {
					request.reject(new Error(message.error.message));
				} else {
					request.resolve(message.result);
				}
			}
		});
		socket.addEventListener('close', () => {
			this.dispose();
		});
	}

	public request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
		const id = ++this.#nextId;
		return new Promise<T>((resolve, reject) => {
			const timer = setTimeout(() => {
				this.#pending.delete(id);
				reject(new Error(`${method} timed out`));
			}, 10000);
			this.#pending.set(id, {
				resolve: value => {
					resolve(value as T);
				}, reject, timer
			});
			try {
				if (!this.#socket) {
					throw new Error('DevTools is not connected');
				}
				this.#socket.send(JSON.stringify({ id, method, params }));
			} catch (error) {
				clearTimeout(timer);
				this.#pending.delete(id);
				reject(error instanceof Error ? error : new Error(String(error)));
			}
		});
	}

	public async evaluate<T = unknown>(expression: string): Promise<T> {
		const reply = await this.request<{ result: { value: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
		if (reply.exceptionDetails) {
			throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
		}
		return reply.result.value;
	}

	public dispose(): void {
		for (const request of this.#pending.values()) {
			clearTimeout(request.timer);
			request.reject(new Error('DevTools connection closed'));
		}
		this.#pending.clear();
		this.#socket?.close();
	}
}
