import assert from 'node:assert/strict';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { contributingWithPrerequisites } from '../../build/lib/prerequisitesDocs.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('CONTRIBUTING.md carries the setup section generated from the manifest', async () => {
	const { current, updated } = await contributingWithPrerequisites(root);
	assert.equal(current, updated, 'Run `node build/run.ts docs:prerequisites`');
	assert.doesNotMatch(current, /requires the MSVC toolchain/);
	assert.doesNotMatch(current, /\]\(docs\//, 'docs/ is ignored and not published');
	assert.match(current, /sudo dnf install .*pipewire-devel/);
	assert.match(current, /Intel Macs cannot build VniLink/);
	// A second computer is listed as optional, never as a need, and nothing reaches it over SSH.
	for (const line of current.split('\n').filter(line => /^\| `npm run measure:(latency|perceived)` \|/.test(line))) {
		assert.match(line, /A second computer \(optional\)/, line);
		assert.doesNotMatch(line, /OpenSSH/, line);
	}
	assert.doesNotMatch(current, /\.; /, 'joined steps do not end their own sentences');
});
