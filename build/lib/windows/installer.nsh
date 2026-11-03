# ---------------------------------------------------------------------------------------------
# Copyright (c) 2026 Sudosy Labs. All rights reserved.
# Licensed under the GNU Affero General Public License, version 3 only.
# See LICENSE in the project root for license information.
# SPDX-License-Identifier: AGPL-3.0-only
# ---------------------------------------------------------------------------------------------

# What VniLink's assisted setup adds to electron-builder's pages (package.win32.ts
# includes it): a welcome page over the sidebar artwork; the license, shown as
# information, since it is not an agreement to accept, so its button goes on
# rather than agreeing; the folder; a page offering a desktop shortcut, off
# unless chosen; then the finish page, which offers to start VniLink. Starting
# at sign-in stays a setting of the app.

!include nsDialogs.nsh
!include LogicLib.nsh

!define MUI_LICENSEPAGE_BUTTON "$(^NextBtn)"
!define MUI_LICENSEPAGE_TEXT_TOP "VniLink is free software, licensed under the GNU Affero General Public License."
!define MUI_LICENSEPAGE_TEXT_BOTTOM "You do not need to accept this license to install or use VniLink: it sets out your rights to share and change it. The licenses of the components VniLink includes are in the licenses folder where it is installed."

# VniLink installs for the current user without elevation, so the page asking who
# to install for is skipped, in setup and in the uninstaller.
!macro customInstallMode
	StrCpy $isForceCurrentInstall "1"
!macroend

# electron-builder compiles the uninstaller on its own, where these would go unreferenced.
!ifndef BUILD_UNINSTALLER
	!macro customWelcomePage
		!define MUI_WELCOMEPAGE_TITLE "Welcome to VniLink"
		!define MUI_WELCOMEPAGE_TEXT "Setup installs VniLink for you on this computer, without asking for administrator rights.$\r$\n$\r$\nVniLink connects your computers, so you can reach one from another.$\r$\n$\r$\nClick Next to continue."
		!insertmacro MUI_PAGE_WELCOME
	!macroend

	Var VniLinkDesktopShortcut
	Var VniLinkDesktopCheckbox

	!macro customInit
		StrCpy $VniLinkDesktopShortcut ${BST_UNCHECKED}
	!macroend

	!macro customInstall
		${If} $VniLinkDesktopShortcut == ${BST_CHECKED}
			CreateShortCut "$DESKTOP\${SHORTCUT_NAME}.lnk" "$INSTDIR\${APP_EXECUTABLE_FILENAME}"
		${EndIf}
	!macroend

	# The page's functions are defined where electron-builder inserts the page,
	# after the Modern UI is included: this file is included before it.
	!macro customPageAfterChangeDir
		Page custom VniLinkTasksPage VniLinkTasksLeave

		Function VniLinkTasksPage
			!insertmacro MUI_HEADER_TEXT "Additional tasks" "Choose what else setup does."
			nsDialogs::Create 1018
			Pop $0
			${NSD_CreateCheckbox} 0 0 100% 12u "Create a desktop shortcut"
			Pop $VniLinkDesktopCheckbox
			${NSD_SetState} $VniLinkDesktopCheckbox $VniLinkDesktopShortcut
			nsDialogs::Show
		FunctionEnd

		Function VniLinkTasksLeave
			${NSD_GetState} $VniLinkDesktopCheckbox $VniLinkDesktopShortcut
		FunctionEnd
	!macroend
!endif

!macro customUnInstall
	Delete "$DESKTOP\${SHORTCUT_NAME}.lnk"
!macroend
