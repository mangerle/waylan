import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

export default class WaylanHideInputExtension extends Extension {
    enable() {
        this._indicator = Main.panel.statusArea['keyboard'];
        if (!this._indicator) {
            return;
        }

        // 保存原始 show 方法以便在扩展禁用时完美还原
        this._origShow = this._indicator.show;
        this._indicator.show = () => {
            // 拦截 GNOME Shell 内部 _currentSourceChanged 对 show() 的调用
        };
        this._indicator.hide();

        if (this._indicator.container) {
            this._origContainerShow = this._indicator.container.show;
            this._indicator.container.show = () => {};
            this._indicator.container.hide();
        }

        // 监听 visible 属性变更，防止 GNOME 内部直接通过 visible = true 绕过 show()
        this._visibleWatchId = this._indicator.connect('notify::visible', () => {
            if (this._indicator && this._indicator.visible) {
                this._indicator.visible = false;
            }
        });

        if (this._indicator.container) {
            this._containerVisibleWatchId = this._indicator.container.connect('notify::visible', () => {
                if (this._indicator && this._indicator.container && this._indicator.container.visible) {
                    this._indicator.container.visible = false;
                }
            });
        }

        // 自动激活微澜为首选活跃输入源（解决开机/重启时 GNOME Shell 因 IBus 注册时差自动回退到 xkb 英文布局的问题）
        this._manager = this._indicator._inputSourceManager;
        if (this._manager) {
            this._activateWaylanIfAvailable();
            this._sourcesChangedId = this._manager.connect('sources-changed', () => {
                this._activateWaylanIfAvailable();
            });
        }
    }

    _activateWaylanIfAvailable() {
        if (!this._manager) {
            return;
        }
        const current = this._manager.currentSource;
        if (current && current.id === 'waylan') {
            return;
        }
        const sources = this._manager.inputSources;
        for (const i in sources) {
            const is = sources[i];
            if (is && is.id === 'waylan') {
                is.activate(true);
                break;
            }
        }
    }

    disable() {
        if (this._manager && this._sourcesChangedId) {
            this._manager.disconnect(this._sourcesChangedId);
            this._sourcesChangedId = 0;
            this._manager = null;
        }

        if (!this._indicator) {
            this._indicator = Main.panel.statusArea['keyboard'];
        }
        if (!this._indicator) {
            return;
        }

        // 断开属性变动监听信号
        if (this._visibleWatchId) {
            this._indicator.disconnect(this._visibleWatchId);
            this._visibleWatchId = 0;
        }
        if (this._containerVisibleWatchId && this._indicator.container) {
            this._indicator.container.disconnect(this._containerVisibleWatchId);
            this._containerVisibleWatchId = 0;
        }

        // 恢复原始方法与面板展示
        if (this._origShow) {
            this._indicator.show = this._origShow;
            this._origShow = null;
        }
        if (this._origContainerShow && this._indicator.container) {
            this._indicator.container.show = this._origContainerShow;
            this._origContainerShow = null;
        }

        this._indicator.visible = true;
        if (this._indicator.container) {
            this._indicator.container.visible = true;
        }
        this._indicator = null;
    }
}
