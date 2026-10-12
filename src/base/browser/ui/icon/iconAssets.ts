import type { IconId, IconVariant } from '../../../common/icons.js';
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

// The only place that knows the icon provider and its filenames.
const assets: Record<IconId, Record<IconVariant, string>> = {
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
