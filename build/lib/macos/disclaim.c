// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

/*
 * Starts a program that is responsible for itself on macOS.
 *
 * macOS attributes privacy permissions (camera, microphone, screen recording,
 * accessibility) to the app that launched a process, so a development build
 * started from a terminal or an editor would ask for, and show, that app's
 * permissions. Disclaiming responsibility, as Chromium does for the processes
 * it launches, makes the program its own. It stays in this process group and
 * shares its standard streams; this process waits and exits with its status.
 *
 * Usage: disclaim <program> [arguments...]
 */
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

/* libsystem's spawn attribute (spawn_private.h), available since macOS 10.14. */
int responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, int disclaim);

int main(int argc, char **argv) {
	if (argc < 2) {
		fprintf(stderr, "usage: disclaim <program> [arguments...]\n");
		return 64;
	}
	posix_spawnattr_t attrs;
	int error = posix_spawnattr_init(&attrs);
	if (error == 0) {
		error = responsibility_spawnattrs_setdisclaim(&attrs, 1);
	}
	if (error != 0) {
		fprintf(stderr, "disclaim: cannot disclaim responsibility: %s\n", strerror(error));
		return 70;
	}
	pid_t child;
	error = posix_spawn(&child, argv[1], NULL, &attrs, argv + 1, environ);
	posix_spawnattr_destroy(&attrs);
	if (error != 0) {
		fprintf(stderr, "disclaim: %s: %s\n", argv[1], strerror(error));
		return 127;
	}
	int status;
	while (waitpid(child, &status, 0) < 0) {
		if (errno != EINTR) {
			perror("disclaim: waitpid");
			return 70;
		}
	}
	if (WIFEXITED(status)) {
		return WEXITSTATUS(status);
	}
	if (WIFSIGNALED(status)) {
		// End the same way, so the supervisor sees how the program stopped.
		signal(WTERMSIG(status), SIG_DFL);
		raise(WTERMSIG(status));
	}
	return 70;
}
