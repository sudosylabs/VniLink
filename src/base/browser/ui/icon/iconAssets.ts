import type { IconId, IconVariant } from '../../../common/icons.js';
import refreshOutlined from '@phosphor-icons/core/assets/regular/arrow-clockwise.svg';
import refreshFilled from '@phosphor-icons/core/assets/fill/arrow-clockwise-fill.svg';
import homeOutlined from '@phosphor-icons/core/assets/regular/house.svg';
import homeFilled from '@phosphor-icons/core/assets/fill/house-fill.svg';
import computersOutlined from '@phosphor-icons/core/assets/regular/desktop.svg';
import computersFilled from '@phosphor-icons/core/assets/fill/desktop-fill.svg';
import backOutlined from '@phosphor-icons/core/assets/regular/arrow-left.svg';
import backFilled from '@phosphor-icons/core/assets/fill/arrow-left-fill.svg';
import forwardOutlined from '@phosphor-icons/core/assets/regular/arrow-right.svg';
import forwardFilled from '@phosphor-icons/core/assets/fill/arrow-right-fill.svg';
import chevronLeftOutlined from '@phosphor-icons/core/assets/regular/caret-left.svg';
import chevronLeftFilled from '@phosphor-icons/core/assets/fill/caret-left-fill.svg';
import chevronRightOutlined from '@phosphor-icons/core/assets/regular/caret-right.svg';
import chevronRightFilled from '@phosphor-icons/core/assets/fill/caret-right-fill.svg';
import chevronUpOutlined from '@phosphor-icons/core/assets/regular/caret-up.svg';
import chevronUpFilled from '@phosphor-icons/core/assets/fill/caret-up-fill.svg';
import chevronDownOutlined from '@phosphor-icons/core/assets/regular/caret-down.svg';
import chevronDownFilled from '@phosphor-icons/core/assets/fill/caret-down-fill.svg';
import settingsOutlined from '@phosphor-icons/core/assets/regular/gear.svg';
import settingsFilled from '@phosphor-icons/core/assets/fill/gear-fill.svg';
import closeOutlined from '@phosphor-icons/core/assets/regular/x.svg';
import closeFilled from '@phosphor-icons/core/assets/fill/x-fill.svg';
import minimizeOutlined from '@phosphor-icons/core/assets/regular/minus.svg';
import minimizeFilled from '@phosphor-icons/core/assets/fill/minus-fill.svg';
import maximizeOutlined from '@phosphor-icons/core/assets/regular/square.svg';
import maximizeFilled from '@phosphor-icons/core/assets/fill/square-fill.svg';
import restoreOutlined from '@phosphor-icons/core/assets/regular/copy.svg';
import restoreFilled from '@phosphor-icons/core/assets/fill/copy-fill.svg';
import connectionOutlined from '@phosphor-icons/core/assets/regular/plugs.svg';
import connectionFilled from '@phosphor-icons/core/assets/fill/plugs-fill.svg';
import securityOutlined from '@phosphor-icons/core/assets/regular/shield-check.svg';
import securityFilled from '@phosphor-icons/core/assets/fill/shield-check-fill.svg';
import keyboardOutlined from '@phosphor-icons/core/assets/regular/keyboard.svg';
import keyboardFilled from '@phosphor-icons/core/assets/fill/keyboard-fill.svg';
import filesOutlined from '@phosphor-icons/core/assets/regular/folder.svg';
import filesFilled from '@phosphor-icons/core/assets/fill/folder-fill.svg';
import terminalOutlined from '@phosphor-icons/core/assets/regular/terminal-window.svg';
import terminalFilled from '@phosphor-icons/core/assets/fill/terminal-window-fill.svg';
import audioOutlined from '@phosphor-icons/core/assets/regular/speaker-high.svg';
import audioFilled from '@phosphor-icons/core/assets/fill/speaker-high-fill.svg';
import recordingOutlined from '@phosphor-icons/core/assets/regular/record.svg';
import recordingFilled from '@phosphor-icons/core/assets/fill/record-fill.svg';
import powerOutlined from '@phosphor-icons/core/assets/regular/power.svg';
import powerFilled from '@phosphor-icons/core/assets/fill/power-fill.svg';
import devicesOutlined from '@phosphor-icons/core/assets/regular/devices.svg';
import devicesFilled from '@phosphor-icons/core/assets/fill/devices-fill.svg';
import remoteDesktopOutlined from '@phosphor-icons/core/assets/regular/computer-tower.svg';
import remoteDesktopFilled from '@phosphor-icons/core/assets/fill/computer-tower-fill.svg';
import appearanceOutlined from '@phosphor-icons/core/assets/regular/palette.svg';
import appearanceFilled from '@phosphor-icons/core/assets/fill/palette-fill.svg';
import appleOutlined from '@phosphor-icons/core/assets/regular/apple-logo.svg';
import appleFilled from '@phosphor-icons/core/assets/fill/apple-logo-fill.svg';
import windowsOutlined from '@phosphor-icons/core/assets/regular/windows-logo.svg';
import windowsFilled from '@phosphor-icons/core/assets/fill/windows-logo-fill.svg';
import linuxOutlined from '@phosphor-icons/core/assets/regular/linux-logo.svg';
import linuxFilled from '@phosphor-icons/core/assets/fill/linux-logo-fill.svg';
import importInvitationOutlined from '@phosphor-icons/core/assets/regular/download-simple.svg';
import importInvitationFilled from '@phosphor-icons/core/assets/fill/download-simple-fill.svg';
import createInvitationOutlined from '@phosphor-icons/core/assets/regular/export.svg';
import createInvitationFilled from '@phosphor-icons/core/assets/fill/export-fill.svg';
import viewOutlined from '@phosphor-icons/core/assets/regular/eye.svg';
import viewFilled from '@phosphor-icons/core/assets/fill/eye-fill.svg';
import cameraOutlined from '@phosphor-icons/core/assets/regular/camera.svg';
import cameraFilled from '@phosphor-icons/core/assets/fill/camera-fill.svg';
import lockOutlined from '@phosphor-icons/core/assets/regular/lock-simple.svg';
import lockFilled from '@phosphor-icons/core/assets/fill/lock-simple-fill.svg';
import fullScreenOutlined from '@phosphor-icons/core/assets/regular/corners-out.svg';
import fullScreenFilled from '@phosphor-icons/core/assets/fill/corners-out-fill.svg';
import warningOutlined from '@phosphor-icons/core/assets/regular/warning.svg';
import warningFilled from '@phosphor-icons/core/assets/fill/warning-fill.svg';
import infoOutlined from '@phosphor-icons/core/assets/regular/info.svg';
import infoFilled from '@phosphor-icons/core/assets/fill/info-fill.svg';
import errorOutlined from '@phosphor-icons/core/assets/regular/warning-octagon.svg';
import errorFilled from '@phosphor-icons/core/assets/fill/warning-octagon-fill.svg';
import exitFullScreenOutlined from '@phosphor-icons/core/assets/regular/corners-in.svg';
import exitFullScreenFilled from '@phosphor-icons/core/assets/fill/corners-in-fill.svg';
import fillViewOutlined from '@phosphor-icons/core/assets/regular/arrows-out-simple.svg';
import fillViewFilled from '@phosphor-icons/core/assets/fill/arrows-out-simple-fill.svg';
import fitToViewOutlined from '@phosphor-icons/core/assets/regular/arrows-in-simple.svg';
import fitToViewFilled from '@phosphor-icons/core/assets/fill/arrows-in-simple-fill.svg';
import stopOutlined from '@phosphor-icons/core/assets/regular/stop.svg';
import stopFilled from '@phosphor-icons/core/assets/fill/stop-fill.svg';

// The only place that knows the icon provider and its filenames.
const assets: Record<IconId, Record<IconVariant, string>> = {
	apple: { outlined: appleOutlined, filled: appleFilled },
	windows: { outlined: windowsOutlined, filled: windowsFilled },
	linux: { outlined: linuxOutlined, filled: linuxFilled },
	appearance: { outlined: appearanceOutlined, filled: appearanceFilled },
	connection: { outlined: connectionOutlined, filled: connectionFilled },
	security: { outlined: securityOutlined, filled: securityFilled },
	keyboard: { outlined: keyboardOutlined, filled: keyboardFilled },
	files: { outlined: filesOutlined, filled: filesFilled },
	terminal: { outlined: terminalOutlined, filled: terminalFilled },
	audio: { outlined: audioOutlined, filled: audioFilled },
	recording: { outlined: recordingOutlined, filled: recordingFilled },
	power: { outlined: powerOutlined, filled: powerFilled },
	importInvitation: { outlined: importInvitationOutlined, filled: importInvitationFilled },
	createInvitation: { outlined: createInvitationOutlined, filled: createInvitationFilled },
	view: { outlined: viewOutlined, filled: viewFilled },
	camera: { outlined: cameraOutlined, filled: cameraFilled },
	lock: { outlined: lockOutlined, filled: lockFilled },
	copy: { outlined: restoreOutlined, filled: restoreFilled },
	fullScreen: { outlined: fullScreenOutlined, filled: fullScreenFilled },
	exitFullScreen: { outlined: exitFullScreenOutlined, filled: exitFullScreenFilled },
	warning: { outlined: warningOutlined, filled: warningFilled },
	info: { outlined: infoOutlined, filled: infoFilled },
	error: { outlined: errorOutlined, filled: errorFilled },
	fillView: { outlined: fillViewOutlined, filled: fillViewFilled },
	fitToView: { outlined: fitToViewOutlined, filled: fitToViewFilled },
	stop: { outlined: stopOutlined, filled: stopFilled },
	devices: { outlined: devicesOutlined, filled: devicesFilled },
	remoteDesktop: { outlined: remoteDesktopOutlined, filled: remoteDesktopFilled },
	refresh: { outlined: refreshOutlined, filled: refreshFilled },
	restart: { outlined: refreshOutlined, filled: refreshFilled },
	home: { outlined: homeOutlined, filled: homeFilled },
	computers: { outlined: computersOutlined, filled: computersFilled },
	back: { outlined: backOutlined, filled: backFilled },
	forward: { outlined: forwardOutlined, filled: forwardFilled },
	chevronLeft: { outlined: chevronLeftOutlined, filled: chevronLeftFilled },
	chevronRight: { outlined: chevronRightOutlined, filled: chevronRightFilled },
	chevronUp: { outlined: chevronUpOutlined, filled: chevronUpFilled },
	chevronDown: { outlined: chevronDownOutlined, filled: chevronDownFilled },
	settings: { outlined: settingsOutlined, filled: settingsFilled },
	close: { outlined: closeOutlined, filled: closeFilled },
	minimize: { outlined: minimizeOutlined, filled: minimizeFilled },
	maximize: { outlined: maximizeOutlined, filled: maximizeFilled },
	restore: { outlined: restoreOutlined, filled: restoreFilled },
};

export function iconSvg(icon: IconId, variant: IconVariant): string {
	return assets[icon][variant];
}
