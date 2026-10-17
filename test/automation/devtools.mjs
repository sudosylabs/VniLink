/** Small owned DevTools connection used only by the external packaged-app verifier. */
export class DevToolsConnection {
	#nextId = 0;
	#pending = new Map();

	async connect(url) {
		this.socket = new WebSocket(url);
		await new Promise((resolve, reject) => {
			const timer = setTimeout(() => reject(new Error('DevTools connection timed out')), 10000);
			this.socket.addEventListener('open', () => {
				clearTimeout(timer);
				resolve();
			}, { once: true });
			this.socket.addEventListener('error', () => {
				clearTimeout(timer);
				reject(new Error('DevTools connection failed'));
			}, { once: true });
		});
		this.socket.addEventListener('message', event => {
			const message = JSON.parse(String(event.data));
			const request = this.#pending.get(message.id);
			if (request) {
				this.#pending.delete(message.id);
				clearTimeout(request.timer);
				if (message.error) {
					request.reject(new Error(message.error.message));
				} else {
					request.resolve(message.result);
				}
			}
		});
		this.socket.addEventListener('close', () => this.dispose());
	}

	request(method, params = {}) {
		const id = ++this.#nextId;
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.#pending.delete(id);
				reject(new Error(`${method} timed out`));
			}, 10000);
			this.#pending.set(id, { resolve, reject, timer });
			try {
				this.socket.send(JSON.stringify({ id, method, params }));
			} catch (error) {
				clearTimeout(timer);
				this.#pending.delete(id);
				reject(error);
			}
		});
	}

	async evaluate(expression) {
		const reply = await this.request('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
		if (reply.exceptionDetails) {
			throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
		}
		return reply.result.value;
	}

	dispose() {
		for (const request of this.#pending.values()) {
			clearTimeout(request.timer);
			request.reject(new Error('DevTools connection closed'));
		}
		this.#pending.clear();
		this.socket?.close();
	}
}
