@echo off
REM ---------------------------------------------------------------------------------------------
REM Copyright (c) 2026 Sudosy Labs. All rights reserved.
REM Licensed under the GNU Affero General Public License, version 3 only.
REM See LICENSE in the project root for license information.
REM SPDX-License-Identifier: AGPL-3.0-only
REM ---------------------------------------------------------------------------------------------

setlocal

:: Launches the development application from a checkout in any state, after
:: VS Code's scripts\code.bat: build\lib\preLaunch.ts installs and builds what
:: is missing, then `node build\run.ts dev` checks what the launch needs,
:: prepares Electron and starts it with the watchers. Arguments go to the app.

title VniLink Dev

pushd %~dp0\..

node build/lib/preLaunch.ts || (
	echo Failed to prepare VniLink for launch ^(build/lib/preLaunch.ts^). 1>&2
	goto :failed
)

node build/run.ts dev %*
set EXIT_CODE=%ERRORLEVEL%
popd
endlocal & exit /b %EXIT_CODE%

:failed
popd
endlocal
exit /b 1
