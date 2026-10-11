import { spawn } from 'node:child_process';
import electron from 'electron';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const child = spawn(electron, [root, ...process.argv.slice(2)], { stdio: 'inherit', cwd: root });
child.on('error', (error) => { console.error(error); process.exitCode = 1; });
child.on('exit', (code, signal) => { process.exitCode = code ?? (signal ? 1 : 0); });
for (const signal of ['SIGINT', 'SIGTERM']) { process.on(signal, () => child.kill(signal)); }
