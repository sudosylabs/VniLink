import assert from 'node:assert/strict';
import test from 'node:test';
import { checkoutVolume, linuxValidationVolumes } from '../../build/lib/containers.ts';

test('each checkout caches its build in a volume of its own', () => {
	const main = checkoutVolume('vnilink-impaired-release-target', '/work/vnilink');
	const worktree = checkoutVolume('vnilink-impaired-release-target', '/work/vnilink/.claude/worktrees/one');
	assert.match(main, /^vnilink-impaired-release-target-[0-9a-f]{12}$/);
	assert.notEqual(main, worktree);
	assert.equal(main, checkoutVolume('vnilink-impaired-release-target', '/work/vnilink'));
});

test('Linux validation of two worktrees shares only the registry', () => {
	const sources = root => linuxValidationVolumes(root).filter(argument => argument !== '--mount').map(mount => mount.match(/source=([^,]+),target=([^,]+)$/)?.slice(1));
	const main = new Map(sources('/work/vnilink').map(([source, target]) => [target, source]));
	const worktree = new Map(sources('/work/vnilink/.claude/worktrees/one').map(([source, target]) => [target, source]));
	assert.deepEqual([...main.keys()], ['/usr/local/cargo/registry', '/work/target', '/work/node_modules']);
	assert.equal(main.get('/usr/local/cargo/registry'), worktree.get('/usr/local/cargo/registry'));
	// Doctests link against the target's libraries; another checkout's build must not replace them mid-run.
	assert.notEqual(main.get('/work/target'), worktree.get('/work/target'));
	assert.notEqual(main.get('/work/node_modules'), worktree.get('/work/node_modules'));
});
