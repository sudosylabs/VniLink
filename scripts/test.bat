@echo off
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
