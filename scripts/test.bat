@echo off
REM ---------------------------------------------------------------------------------------------
REM Copyright (c) 2026 Sudosy Labs. All rights reserved.
REM Licensed under the GNU Affero General Public License, version 3 only.
REM See LICENSE in the project root for license information.
REM SPDX-License-Identifier: AGPL-3.0-only
REM ---------------------------------------------------------------------------------------------

setlocal

:: Runs the UI scenarios from a checkout in any state, after VS Code's
:: scripts\test.bat: build\lib\preLaunch.ts installs and builds what is
:: missing, then `node build\run.ts test:ui` runs the scenarios. Arguments
:: select them, for example `scripts\test.bat --scenario appearance`.

pushd %~dp0\..

node build/lib/preLaunch.ts || (
	echo Failed to prepare VniLink for the tests ^(build/lib/preLaunch.ts^). 1>&2
	popd
	endlocal
	exit /b 1
)

node build/run.ts test:ui %*
set EXIT_CODE=%ERRORLEVEL%
popd
endlocal & exit /b %EXIT_CODE%
