/**
 * `npm run doctor`: checks everything `build/prerequisites.json` lists for
 * this OS and reports each task as ready, missing what, or unsupported here.
 *
 *   --task <name>      only these tasks (repeatable)
 *   --require <names>  exit 1 unless these comma-separated tasks are ready (CI)
 *   --json             the reports as JSON
 */
import { homedir } from 'node:os';
import { checkTasks, detectHost, firstBuildDownloads, fixLines, loadManifest, systemProbe, type CheckResult, type Probe, type TaskReport } from './prerequisites.ts';

interface DoctorOptions {
	readonly tasks: readonly string[];
	readonly required: readonly string[];
	readonly json: boolean;
}

function parse(args: readonly string[]): DoctorOptions {
	const tasks: string[] = [];
	const required: string[] = [];
	let json = false;
	for (let index = 0; index < args.length; index++) {
		const argument = args[index];
		if (argument === '--json') {
			json = true;
		} else if ((argument === '--task' || argument === '--require') && args[index + 1]) {
			const names = args[++index].split(',').filter(Boolean);
			(argument === '--task' ? tasks : required).push(...names);
		} else {
			throw new Error(`Unknown doctor argument ${argument}. Use --task <name>, --require <names> or --json.`);
		}
	}
	return { tasks, required, json };
}

const labels: Readonly<Record<CheckResult['status'], string>> = { ready: 'ok', missing: 'MISSING', unsupported: 'unsupported', unknown: 'unknown' };

function requirementLine(result: CheckResult): string {
	const label = result.status === 'missing' && result.optional ? 'optional' : labels[result.status];
	return `  ${label.padEnd(12)}${result.title}${result.detail ? `: ${result.detail}` : ''}`;
}

/** The report as text: every requirement once, then every task by status. */
export function formatDoctor(reports: readonly TaskReport[], header: string, fixes: readonly string[], downloads: readonly string[]): string {
	const requirements = new Map<string, CheckResult>();
	for (const report of reports) {
		for (const result of report.results) {
			requirements.set(result.id, result);
		}
	}
	const lines = [header, '', 'Requirements'];
	for (const result of requirements.values()) {
		lines.push(requirementLine(result));
		if (result.note) {
			lines.push(`  ${''.padEnd(12)}note: ${result.note}`);
		}
	}
	lines.push('', 'Tasks');
	for (const status of ['ready', 'missing', 'unsupported'] as const) {
		const selected = reports.filter(report => report.status === status);
		if (!selected.length) {
			continue;
		}
		if (status === 'ready') {
			lines.push(`  ${'ready'.padEnd(12)}${selected.map(report => report.name).join(', ')}`);
		} else {
			for (const report of selected) {
				lines.push(`  ${(status === 'missing' ? 'missing' : 'unsupported').padEnd(12)}${report.name}: ${report.reason ?? ''}`);
			}
		}
	}
	if (fixes.length) {
		lines.push('', 'To fix', ...fixes);
	}
	if (downloads.length) {
		lines.push('', 'The first native build downloads, once:', ...downloads.map(download => `  ${download}`));
	}
	return lines.join('\n');
}

export async function doctor(root: string, args: readonly string[], probe: Probe = systemProbe): Promise<TaskReport[]> {
	const options = parse(args);
	const manifest = await loadManifest();
	const unknown = [...options.tasks, ...options.required].filter(name => !Object.hasOwn(manifest.tasks, name));
	if (unknown.length) {
		throw new Error(`build/prerequisites.json has no task ${unknown.join(', ')}`);
	}
	const host = await detectHost(root, manifest, probe);
	const names = options.tasks.length ? [...new Set([...options.tasks, ...options.required])] : Object.keys(manifest.tasks);
	const reports = await checkTasks(manifest, host, probe, names);
	if (options.json) {
		const { osName: os, platform, architecture, hardwareArchitecture, nodeVersion: node, distribution = null } = host;
		console.log(JSON.stringify({ host: { os, platform, architecture, hardwareArchitecture, node, distribution }, tasks: reports }, null, 2));
	} else {
		const missing = new Map<string, CheckResult>();
		for (const report of reports) {
			for (const result of report.results) {
				if (result.status === 'missing' || result.status === 'unknown') {
					missing.set(result.id, result);
				}
			}
		}
		const downloads = (await firstBuildDownloads(manifest, host.platform, root, homedir(), probe)).map(download => `${download.what} into ${download.into}`);
		const header = `VniLink doctor: ${host.osName} ${host.hardwareArchitecture}, Node ${host.nodeVersion} ${host.architecture}${host.distribution ? `, ${manifest.platforms.linux.distributions?.[host.distribution].name ?? host.distribution} packages` : ''}`;
		console.log(formatDoctor(reports, header, fixLines(manifest, host, [...missing.values()]), downloads));
	}
	const failed = reports.filter(report => options.required.includes(report.name) && report.status !== 'ready');
	if (failed.length) {
		process.exitCode = 1;
		console.error(`\nNot ready: ${failed.map(report => report.name).join(', ')}`);
	}
	return reports;
}
