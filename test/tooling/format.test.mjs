import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatTypeScript } from '../../build/format.ts';

test('TypeScript formatting uses tabs, is idempotent and preserves literal whitespace', () => {
	const input = 'function example(){\n  const text = `line with spaces  \nnext line`;\n  return text;\n}\n';
	const formatted = formatTypeScript('/tmp/format-fixture.ts', input);
	assert.match(formatted, /\n\tconst text/);
	assert.ok(formatted.includes('line with spaces  \nnext line'));
	assert.equal(formatTypeScript('/tmp/format-fixture.ts', formatted), formatted);
});

test('formatting expands compressed ownership code without rewriting literal content', () => {
	const input = 'class Owner { value = "a; b"; start() { const one = 1; const two = 2; return [one,two]; } stop() { return this.value; } }';
	const formatted = formatTypeScript('/tmp/owner.ts', input);
	assert.match(formatted, /const one = 1;\n\s+const two = 2;/);
	assert.match(formatted, /\n\n\s+stop/);
	assert(formatted.includes('"a; b"'));
	assert.equal(formatTypeScript('/tmp/owner.ts', formatted), formatted);
});

test('formatting keeps catch, finally and else beside the preceding block', () => {
	const input = 'try { work(); }\ncatch (error) { fail(error); }\nfinally { stop(); }\nif (ready) { work(); }\nelse { stop(); }';
	const formatted = formatTypeScript('/tmp/control.ts', input);
	assert.match(formatted, /} catch/);
	assert.match(formatted, /} finally/);
	assert.match(formatted, /} else/);
	assert.equal(formatTypeScript('/tmp/control.ts', formatted), formatted);
});
