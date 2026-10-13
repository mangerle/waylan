import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

export default class WaylanHideInputExtension extends Extension {
    enable() {
        this._indicator = Main.panel.statusArea['keyboard'];
        if (this._indicator) {
            this._indicator.visible = false;
            if (this._indicator.container) {
                this._indicator.container.visible = false;
            }
        }
    }

    disable() {
        if (this._indicator) {
            this._indicator.visible = true;
            if (this._indicator.container) {
                this._indicator.container.visible = true;
            }
            this._indicator = null;
        } else {
            const indicator = Main.panel.statusArea['keyboard'];
            if (indicator) {
                indicator.visible = true;
                if (indicator.container) {
                    indicator.container.visible = true;
                }
            }
        }
    }
}
