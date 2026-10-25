import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import { join, relative } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

/** The line a Rust module carries once every failure it ignores is recorded (`vnilink::trace::ignored!`). */
const MARKER = '// trace: records every ignored failure';

async function rustFiles(folder) {
	const entries = await readdir(folder, { recursive: true, withFileTypes: true });
	return entries.filter(entry => entry.isFile() && entry.name.endsWith('.rs')).map(entry => join(entry.parentPath, entry.name));
}

/** The module's code before its first test module, where a dropped result goes unrecorded. */
function productionCode(text) {
	const tests = text.search(/^\s*#\[cfg\(test\)\]\s*\n\s*(pub(\([a-z]+\))? )?mod /m);
	return tests < 0 ? text : text.slice(0, tests);
}

/** Lines that drop a result without recording it. */
export function silentDrops(text) {
	return productionCode(text)
		.split('\n')
		.map((line, index) => ({ line: line.trim(), number: index + 1 }))
		.filter(({ line }) => !line.startsWith('//') && (/\blet _ = /.test(line) || /\.ok\(\);/.test(line)));
}

test('a dropped result is found, and test modules and comments are not', () => {
	const text = ['// let _ = in a comment', 'let _ = sender.send(1);', 'value.ok();', 'keep(value.ok());', '#[cfg(test)]', 'mod tests {', 'let _ = fixture();', '}'].join('\n');
	assert.deepEqual(silentDrops(text).map(drop => drop.number), [2, 3]);
});

test('modules that record every ignored failure drop nothing silently', async () => {
	const files = await rustFiles(join(root, 'crates'));
	const marked = [];
	const offending = [];
	for (const file of files) {
		const text = await readFile(file, 'utf8');
		if (!text.includes(MARKER)) {
			continue;
		}
		marked.push(file);
		for (const drop of silentDrops(text)) {
			offending.push(`${relative(root, file)}:${drop.number}: ${drop.line}`);
		}
	}
	assert.ok(marked.length > 0, `no module carries "${MARKER}"`);
	assert.deepEqual(offending, [], `record these with vnilink::ignored! and a reason:\n${offending.join('\n')}`);
});
