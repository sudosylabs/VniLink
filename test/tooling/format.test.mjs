import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatTypeScript } from '../../build/format.mjs';
test('TypeScript formatting uses tabs, is idempotent and preserves literal whitespace', () => {
	const input = 'function example(){\n  const text = `line with spaces  \nnext line`;\n  return text;\n}\n';
	const formatted = formatTypeScript('/tmp/format-fixture.ts', input);
	assert.match(formatted, /\n\tconst text/);
	assert.ok(formatted.includes('line with spaces  \nnext line'));
	assert.equal(formatTypeScript('/tmp/format-fixture.ts', formatted), formatted);
});
