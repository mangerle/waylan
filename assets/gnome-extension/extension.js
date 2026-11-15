import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as IBusManager from 'resource:///org/gnome/shell/misc/ibusManager.js';
import Clutter from 'gi://Clutter';
import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

/**
 * 读取微澜输入法配置文件 (~/.config/waylan/config.toml)
 *
 * 轻量提取候选框渲染模式、排版密度、主题预设与语音交互风格
 */
function readWaylanConfig() {
    const configDir = GLib.get_user_config_dir();
    const configPath = GLib.build_filenamev([configDir, 'waylan', 'config.toml']);
    const file = Gio.File.new_for_path(configPath);

    const defaultConfig = {
        windowMode: 'waylan_custom',
        density: 'normal',
        theme: 'emerald',
        voiceVisual: 'dynamic_waveform',
        itemSpacing: 8,
        paddingV: 4,
        paddingH: 8,
        cornerRadius: 8,
        opacity: 95,
        hideIndicator: false,
    };

    if (!file.query_exists(null)) {
        return defaultConfig;
    }

    try {
        const [ok, bytes] = file.load_contents(null);
        if (!ok || !bytes) {
            return defaultConfig;
        }
        const text = new TextDecoder().decode(bytes);

        let windowMode = defaultConfig.windowMode;
        let density = defaultConfig.density;
        let theme = defaultConfig.theme;
        let voiceVisual = defaultConfig.voiceVisual;
        let itemSpacing = defaultConfig.itemSpacing;
        let paddingV = defaultConfig.paddingV;
        let paddingH = defaultConfig.paddingH;
        let cornerRadius = defaultConfig.cornerRadius;
        let opacity = defaultConfig.opacity;

        const modeMatch = text.match(/^\s*candidate_window_mode\s*=\s*"([^"]+)"/m);
        if (modeMatch) {
            windowMode = modeMatch[1];
        }

        const densityMatch = text.match(/^\s*candidate_density\s*=\s*"([^"]+)"/m);
        if (densityMatch) {
            density = densityMatch[1];
        }

        const themeMatch = text.match(/^\s*candidate_theme\s*=\s*"([^"]+)"/m);
        if (themeMatch) {
            theme = themeMatch[1];
        }

        const voiceMatch = text.match(/^\s*visual_effect\s*=\s*"([^"]+)"/m);
        if (voiceMatch) {
            voiceVisual = voiceMatch[1];
        }

        const spacingMatch = text.match(/^\s*candidate_item_spacing\s*=\s*([0-9.]+)/m);
        if (spacingMatch) {
            itemSpacing = parseFloat(spacingMatch[1]);
        }

        const padVMatch = text.match(/^\s*candidate_padding_v\s*=\s*([0-9.]+)/m);
        if (padVMatch) {
            paddingV = parseFloat(padVMatch[1]);
        }

        const padHMatch = text.match(/^\s*candidate_padding_h\s*=\s*([0-9.]+)/m);
        if (padHMatch) {
            paddingH = parseFloat(padHMatch[1]);
        }

        const radiusMatch = text.match(/^\s*candidate_corner_radius\s*=\s*([0-9.]+)/m);
        if (radiusMatch) {
            cornerRadius = parseFloat(radiusMatch[1]);
        }

        const opacityMatch = text.match(/^\s*candidate_opacity\s*=\s*([0-9]+)/m);
        if (opacityMatch) {
            opacity = parseInt(opacityMatch[1], 10);
        }

        // 顶栏原生指示器的隐藏属独立职责，仅在微澜独占输入源时执行；
        // 读取失败时保持 false（宁可多显示指示器，也不剥夺用户切换输入源的入口）
        const exclusiveMatch = text.match(/^\s*exclusive_input_source\s*=\s*(true|false)/m);
        const hideIndicator = exclusiveMatch ? exclusiveMatch[1] === 'true' : defaultConfig.hideIndicator;

        return {
            windowMode,
            density,
            theme,
            voiceVisual,
            itemSpacing,
            paddingV,
            paddingH,
            cornerRadius,
            opacity,
            hideIndicator,
        };
    } catch (e) {
        console.error('[WaylanExtension] 读取微澜配置文件异常:', e);
        return defaultConfig;
    }
}

/**
 * 微澜拼音输入法专属现代胶囊候选框组件
 *
 * # 设计原理
 * - 实现初衷：替代 GNOME 原生粗糙臃肿的 IBus 候选黑白框，以统一的翡翠绿高亮与紧凑微排版呈现微澜视觉风格。
 * - 核心优势：原生接入 GNOME Shell Clutter/St 顶层 Chrome 渲染管线，零焦点竞争、零跨进程延迟，天然获得精密的光标吸附与防屏幕溢出能力。
 * - 代价与局限：通过纳管 IBusCandidatePopup 的子节点与 setCandidates 实现非侵入式替换，在禁用扩展或切换为原生模式时可无损还原原生 UI。
 */
class WaylanCandidateView {
    constructor(getPanelService, initialConfig) {
        this._getPanelService = getPanelService;
        this._config = initialConfig || {
            density: 'normal',
            theme: 'emerald',
            voiceVisual: 'dynamic_waveform',
        };

        // 主容器：纵向排布（上方拼音预编辑，下方候选词与翻页）
        this.actor = new St.BoxLayout({
            style_class: 'waylan-candidate-container',
            vertical: true,
            reactive: true,
        });

        // 顶部拼音预编辑栏
        this._preeditBar = new St.BoxLayout({
            style_class: 'waylan-preedit-bar',
            vertical: false,
            visible: false,
        });
        this._badge = new St.Label({
            style_class: 'waylan-badge',
            text: '微澜',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._preeditLabel = new St.Label({
            style_class: 'waylan-preedit-text',
            text: '',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._preeditBar.add_child(this._badge);
        this._preeditBar.add_child(this._preeditLabel);
        this.actor.add_child(this._preeditBar);

        // 主候选词展示行（横向流）
        this._mainRow = new St.BoxLayout({
            style_class: 'waylan-candidates-bar',
            vertical: false,
        });
        this.actor.add_child(this._mainRow);

        // 预先创建 10 个候选条目胶囊
        this._items = [];
        for (let i = 0; i < 10; i++) {
            const idx = i;
            const item = new St.Button({
                style_class: 'waylan-cand-item',
                reactive: true,
                track_hover: true,
                visible: false,
                x_align: Clutter.ActorAlign.FILL,
                x_expand: true,
            });
            const content = new St.BoxLayout({
                style_class: 'waylan-cand-content',
                vertical: false,
                x_align: Clutter.ActorAlign.START,
                y_align: Clutter.ActorAlign.CENTER,
                x_expand: true,
            });
            const indexLabel = new St.Label({
                style_class: 'waylan-cand-index',
                y_align: Clutter.ActorAlign.CENTER,
            });
            const textLabel = new St.Label({
                style_class: 'waylan-cand-text',
                y_align: Clutter.ActorAlign.CENTER,
            });
            const tabBadge = new St.Label({
                style_class: 'waylan-tab-badge',
                text: 'Tab ⇥',
                visible: false,
                y_align: Clutter.ActorAlign.CENTER,
            });
            content.add_child(indexLabel);
            content.add_child(textLabel);
            content.add_child(tabBadge);
            item.set_child(content);

            item.connect('clicked', () => {
                const ps = this._getPanelService();
                if (ps) {
                    ps.candidate_clicked(idx, 1, 0);
                }
            });

            item._indexLabel = indexLabel;
            item._textLabel = textLabel;
            item._tabBadge = tabBadge;
            this._items.push(item);
            this._mainRow.add_child(item);
        }

        // 翻页操作区
        this._pageBar = new St.BoxLayout({
            style_class: 'waylan-page-bar',
            vertical: false,
            visible: false,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._prevBtn = new St.Button({
            style_class: 'waylan-page-btn',
            label: '◀',
            reactive: true,
            track_hover: true,
        });
        this._pageLabel = new St.Label({
            style_class: 'waylan-page-label',
            text: '1/1',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._nextBtn = new St.Button({
            style_class: 'waylan-page-btn',
            label: '▶',
            reactive: true,
            track_hover: true,
        });

        this._prevBtn.connect('clicked', () => {
            const ps = this._getPanelService();
            if (ps) {
                ps.page_up();
            }
        });
        this._nextBtn.connect('clicked', () => {
            const ps = this._getPanelService();
            if (ps) {
                ps.page_down();
            }
        });

        this._pageBar.add_child(this._prevBtn);
        this._pageBar.add_child(this._pageLabel);
        this._pageBar.add_child(this._nextBtn);
        this._mainRow.add_child(this._pageBar);

        // ---------------- 现代语音输入动效卡片 ----------------
        this._voiceCapsule = new St.BoxLayout({
            style_class: 'waylan-voice-capsule',
            vertical: false,
            visible: false,
            y_align: Clutter.ActorAlign.CENTER,
        });


        // 律动声浪容器（5 根声浪柱）
        this._waveContainer = new St.BoxLayout({
            style_class: 'waylan-wave-container',
            vertical: false,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._waveBars = [];
        this._currentHeights = [4, 4, 4, 4, 4];
        this._targetHeights = [4, 4, 4, 4, 4];
        for (let b = 0; b < 5; b++) {
            const bar = new St.Bin({
                style_class: 'waylan-wave-bar',
                height: 4,
                y_align: Clutter.ActorAlign.CENTER,
            });
            this._waveBars.push(bar);
            this._waveContainer.add_child(bar);
        }
        this._voiceCapsule.add_child(this._waveContainer);

        // 流光呼吸圆点
        this._pulseDot = new St.Bin({
            style_class: 'waylan-pulse-dot',
            visible: false,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._voiceCapsule.add_child(this._pulseDot);

        // 语音提示文字标签
        this._voiceLabel = new St.Label({
            style_class: 'waylan-voice-label',
            text: '正在聆听中...',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._voiceCapsule.add_child(this._voiceLabel);

        this.actor.add_child(this._voiceCapsule);

        // 应用排版密度与色彩主题
        this.applyConfig(this._config);

        // 监听并动态同步系统深色/浅色外观模式
        try {
            this._settings = new Gio.Settings({ schema_id: 'org.gnome.desktop.interface' });
            this._syncDarkTheme();
            this._settingsWatchId = this._settings.connect('changed::color-scheme', () => {
                this._syncDarkTheme();
            });
        } catch (e) {
            // 降级忽略
        }
    }

    applyConfig(config) {
        this._config = config;

        // 1. 密度类切换 (density-compact / density-normal / density-spacious)
        this.actor.remove_style_class_name('density-compact');
        this.actor.remove_style_class_name('density-normal');
        this.actor.remove_style_class_name('density-spacious');

        if (config.density === 'compact') {
            this.actor.add_style_class_name('density-compact');
        } else if (config.density === 'spacious') {
            this.actor.add_style_class_name('density-spacious');
        } else {
            this.actor.add_style_class_name('density-normal');
        }

        // 2. 主题类切换 (theme-emerald / theme-monochrome / theme-ice-blue / theme-warm-latte)
        this.actor.remove_style_class_name('theme-emerald');
        this.actor.remove_style_class_name('theme-monochrome');
        this.actor.remove_style_class_name('theme-ice-blue');
        this.actor.remove_style_class_name('theme-warm-latte');

        if (config.theme === 'monochrome') {
            this.actor.add_style_class_name('theme-monochrome');
        } else if (config.theme === 'ice_blue') {
            this.actor.add_style_class_name('theme-ice-blue');
        } else if (config.theme === 'warm_latte') {
            this.actor.add_style_class_name('theme-warm-latte');
        } else {
            this.actor.add_style_class_name('theme-emerald');
        }

        // 3. 高级尺寸微调 (以 inline style 形式动态应用至外层容器与主候选流)
        const padV = config.paddingV !== undefined ? config.paddingV : 4;
        const padH = config.paddingH !== undefined ? config.paddingH : 8;
        const radius = config.cornerRadius !== undefined ? config.cornerRadius : 8;
        const opacity = config.opacity !== undefined ? config.opacity : 95;
        const itemSpacing = config.itemSpacing !== undefined ? config.itemSpacing : 8;

        this.actor.style = `padding: ${padV}px ${padH}px !important; border-radius: ${radius}px !important; opacity: ${opacity / 100.0} !important;`;
        const itemRadius = Math.max(3, Math.round(radius - 2));
        if (this._items) {
            for (const item of this._items) {
                item.style = `border-radius: ${itemRadius}px !important;`;
            }
        }
        if (this._mainRow) {
            this._mainRow.spacing = Math.round(itemSpacing);
        }
    }

    _syncDarkTheme() {
        if (!this._settings) {
            return;
        }
        const scheme = this._settings.get_string('color-scheme');
        if (scheme === 'prefer-dark') {
            this.actor.add_style_class_name('dark');
        } else {
            this.actor.remove_style_class_name('dark');
        }
    }

    updatePreedit(text, visible) {
        const hasText = !!(text && text.trim().length > 0);
        this._preeditLabel.text = hasText ? text : '';
        this._preeditBar.visible = visible && hasText;
    }

    updateCandidates(indexes, candidates, cursorPosition, cursorVisible) {
        // 判断是否为语音录入/结算/完成提示状态
        const isVoiceState = candidates.length > 0 &&
            candidates[0] &&
            (candidates[0].includes('正在聆听')
                || candidates[0].includes('正在识别')
                || candidates[0].includes('识别完成'));

        if (isVoiceState && this._config.voiceVisual !== 'classic_text') {
            const waveform = (indexes && indexes.length > 0) ? indexes[0] : '';
            this._showVoiceVisual(candidates[0], waveform);
            return;
        }

        // 退出语音动效，还原普通候选词
        this._hideVoiceVisual();

        for (let i = 0; i < this._items.length; i++) {
            const item = this._items[i];
            if (i < candidates.length) {
                const hasCustomIndex = indexes && indexes[i] && indexes[i] !== '\u200B';
                if (hasCustomIndex) {
                    item._indexLabel.text = indexes[i];
                    item._indexLabel.visible = true;
                } else {
                    item._indexLabel.text = '';
                    item._indexLabel.visible = false;
                }
                let rawText = candidates[i] || '';
                const isExpandable = rawText.includes('⇥') || rawText.includes('Tab');
                if (isExpandable) {
                    rawText = rawText.replace(/\s*⇥\s*Tab/, '').replace(/⇥/, '').trim();
                    item.add_style_class_name('is-expandable');
                    item._tabBadge.visible = true;
                } else {
                    item.remove_style_class_name('is-expandable');
                    item._tabBadge.visible = false;
                }
                item._textLabel.text = rawText;
                if (cursorVisible && i === cursorPosition) {
                    item.add_style_class_name('selected');
                } else {
                    item.remove_style_class_name('selected');
                }
                item.visible = true;
            } else {
                item.visible = false;
                item.remove_style_class_name('selected');
                item.remove_style_class_name('is-expandable');
                item._tabBadge.visible = false;
            }
        }
    }

    _showVoiceVisual(candidateText, waveform) {
        this._mainRow.visible = false;
        this._voiceCapsule.visible = true;

        // 彻底清洗过滤可能附带的字符，文字标题永远保持纯净
        const cleanText = candidateText.replace(/[\u2580-\u259f\u2500-\u257f\s\-_—~]+$/, '').trim();

        // 识别完成状态：停止动态波形，展示完成态并平滑淡出
        if (cleanText.includes('识别完成')) {
            this._stopVoiceAnimation();
            this._waveContainer.visible = false;
            this._pulseDot.visible = true;
            this._pulseDot.opacity = 255;
            this._voiceLabel.text = '识别完成';
            if (!this._isVoiceCompleted) {
                this._isVoiceCompleted = true;
                this._voiceCapsule.remove_all_transitions();
                this._voiceCapsule.opacity = 255;
                this._clearFadeTimer();
                this._fadeTimerId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 280, () => {
                    this._fadeTimerId = 0;
                    if (this._voiceCapsule && this._voiceCapsule.visible && this._isVoiceCompleted) {
                        this._voiceCapsule.ease({
                            opacity: 0,
                            duration: 420,
                            mode: Clutter.AnimationMode.EASE_OUT_QUAD,
                        });
                    }
                    return GLib.SOURCE_REMOVE;
                });
            }
            return;
        }

        // 若从完成淡出态重新进入语音录音/识别，立即恢复不透明并取消淡出
        this._isVoiceCompleted = false;
        this._clearFadeTimer();
        this._voiceCapsule.remove_all_transitions();
        this._voiceCapsule.opacity = 255;

        // 正在识别结算状态：显示呼吸圆点与结算文案
        if (cleanText.includes('正在识别')) {
            this._stopWaveAnimationOnly();
            this._waveContainer.visible = false;
            this._pulseDot.visible = true;
            this._voiceLabel.text = cleanText || '正在识别...';
            this._startPulseAnimation();
            return;
        }

        // 从 waveform (优先) 或 candidateText 提取 5 根音波柱的真实声音能量
        const waveSource = (waveform && waveform.length > 0) ? waveform : candidateText;
        const levels = [];
        for (let i = 0; i < waveSource.length; i++) {
            const code = waveSource.charCodeAt(i);
            // Unicode 块元素 \u2581 (' ') ~ \u2588 ('█')
            if (code >= 0x2581 && code <= 0x2588) {
                levels.push(code - 0x2581);
            }
        }

        if (levels.length > 0) {
            const lastLevels = levels.slice(-5);
            while (lastLevels.length < 5) {
                lastLevels.unshift(0);
            }
            let anyVoice = false;
            for (let i = 0; i < 5; i++) {
                const lvl = lastLevels[i];
                if (lvl > 0) {
                    anyVoice = true;
                    // 真实能量高度映射：从 7px 到 24px 动态起伏
                    this._targetHeights[i] = Math.round(4 + lvl * 2.8);
                } else {
                    // 静音基线：4px 静态平直
                    this._targetHeights[i] = 4;
                }
            }
            this._isSpeaking = anyVoice;
        } else {
            this._targetHeights = [4, 4, 4, 4, 4];
            this._isSpeaking = false;
        }

        if (this._config.voiceVisual === 'breathing_pulse') {
            // 模式 B：流光呼吸光环微卡片
            this._waveContainer.visible = false;
            this._pulseDot.visible = true;
            this._voiceLabel.text = cleanText || '微澜语音输入中...';
            this._startPulseAnimation();
        } else {
            // 模式 A：现代多频律动声浪胶囊 (默认)
            this._waveContainer.visible = true;
            this._pulseDot.visible = false;
            this._voiceLabel.text = cleanText || '正在聆听中...';
            this._startWaveAnimation();
        }
    }

    _hideVoiceVisual() {
        this._stopVoiceAnimation();
        this._clearFadeTimer();
        this._isVoiceCompleted = false;
        if (this._voiceCapsule) {
            this._voiceCapsule.remove_all_transitions();
            this._voiceCapsule.opacity = 255;
            this._voiceCapsule.visible = false;
        }
        this._targetHeights = [4, 4, 4, 4, 4];
        this._currentHeights = [4, 4, 4, 4, 4];
        for (let i = 0; i < this._waveBars.length; i++) {
            this._waveBars[i].height = 4;
        }
        this._mainRow.visible = true;
    }

    _startWaveAnimation() {
        if (this._waveTimerId) {
            return;
        }
        // 30ms 弹性平滑收敛插值循环 (~33 FPS)
        this._waveTimerId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 30, () => {
            for (let i = 0; i < this._waveBars.length; i++) {
                const target = this._targetHeights[i] || 4;
                const current = this._currentHeights[i] || 4;
                // 上升迅速跟手（冲量 0.55），下降平滑收敛（阻尼 0.2）
                const factor = target > current ? 0.55 : 0.2;
                const next = current + (target - current) * factor;
                const diff = Math.abs(next - target);
                if (diff > 0.15) {
                    this._currentHeights[i] = next;
                    this._waveBars[i].height = Math.round(next);
                } else {
                    this._currentHeights[i] = target;
                    this._waveBars[i].height = target;
                }
            }
            return GLib.SOURCE_CONTINUE;
        });
    }

    _startPulseAnimation() {
        if (this._pulseTimerId) {
            return;
        }
        let pulseOn = false;
        this._pulseTimerId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 400, () => {
            pulseOn = !pulseOn;
            this._pulseDot.opacity = pulseOn ? 255 : 120;
            return GLib.SOURCE_CONTINUE;
        });
    }

    _clearFadeTimer() {
        if (this._fadeTimerId) {
            GLib.source_remove(this._fadeTimerId);
            this._fadeTimerId = 0;
        }
    }

    _stopWaveAnimationOnly() {
        if (this._waveTimerId) {
            GLib.source_remove(this._waveTimerId);
            this._waveTimerId = 0;
        }
    }

    _stopVoiceAnimation() {
        this._stopWaveAnimationOnly();
        if (this._pulseTimerId) {
            GLib.source_remove(this._pulseTimerId);
            this._pulseTimerId = 0;
        }
    }

    updatePages(page, nPages) {
        if (nPages > 1) {
            this._pageBar.visible = true;
            this._pageLabel.text = `${page + 1}/${nPages}`;
        } else {
            this._pageBar.visible = false;
        }
    }

    setOrientation(orientation) {
        this._orientation = orientation;
        // 1 为 IBus.Orientation.VERTICAL（微澜按 TAB 展开二级日期时间/多格式卡片时主动发射该模式）
        const isVertical = (orientation === 1);
        this._mainRow.vertical = isVertical;
        for (const item of this._items) {
            item.x_align = isVertical ? Clutter.ActorAlign.FILL : Clutter.ActorAlign.CENTER;
            if (item.child) {
                item.child.x_align = isVertical ? Clutter.ActorAlign.START : Clutter.ActorAlign.CENTER;
                item.child.x_expand = isVertical;
            }
        }
        if (isVertical) {
            this.actor.add_style_class_name('vertical-mode');
            this.actor.remove_style_class_name('horizontal-mode');
        } else {
            this.actor.add_style_class_name('horizontal-mode');
            this.actor.remove_style_class_name('vertical-mode');
        }
    }

    destroy() {
        this._stopVoiceAnimation();
        this._clearFadeTimer();
        if (this._settings && this._settingsWatchId) {
            this._settings.disconnect(this._settingsWatchId);
            this._settingsWatchId = 0;
            this._settings = null;
        }
        this.actor.destroy();
    }
}

export default class WaylanHideInputExtension extends Extension {
    enable() {
        // 扩展被加载不代表要隐藏顶栏指示器：隐藏指示器与渲染微澜候选框是两项独立职责，
        // 分别由 exclusive_input_source 与 candidate_window_mode 决定，故 enable 只建立
        // 配置通道并应用一次当前意图，具体副作用全部下沉到 _applyCurrentConfig。
        this._indicatorHidden = false;
        this._setupConfigWatcher();
        this._setupActiveWindowTracker();
        this._applyCurrentConfig();
    }

    /**
     * 监听 Mutter 合成器顶层焦点窗口变动，并将当前窗口标识与应用类别实时写入 tmpfs
     *
     * 供微澜输入法核心精准实现“多窗口输入模式独立记忆”与“终端类应用默认英文”双轨感知。
     */
    _setupActiveWindowTracker() {
        if (!global.display) {
            return;
        }

        const syncActiveWindow = () => {
            try {
                const win = global.display.focus_window;
                const runtimeDir = GLib.get_user_runtime_dir();
                const filePath = GLib.build_filenamev([runtimeDir, 'waylan_active_window.json']);
                let data;
                if (win) {
                    const wmClass = win.get_wm_class ? (win.get_wm_class() || '') : '';
                    const winId = win.get_id ? String(win.get_id() || '') : '';
                    const title = win.get_title ? (win.get_title() || '') : '';
                    data = JSON.stringify({ win_id: winId, wm_class: wmClass, title: title });
                } else {
                    data = JSON.stringify({ win_id: '', wm_class: '', title: '' });
                }
                GLib.file_set_contents(filePath, data);
            } catch (e) {
                // 忽略非致命写入异常，防止影响合成器事件循环
            }
        };

        this._focusWindowNotifyId = global.display.connect('notify::focus-window', syncActiveWindow);
        // 初始化时立即同步当前焦点窗口
        syncActiveWindow();
    }

    /**
     * 按配置应用或撤销对顶栏原生输入源指示器的隐藏
     *
     * 幂等设计：配置热切换（用户在设置中心勾选/取消独占输入源）时无需重载扩展即可生效。
     */
    _applyIndicatorVisibility(hide) {
        if (!this._indicator) {
            this._indicator = Main.panel.statusArea['keyboard'];
        }
        if (!this._indicator || hide === this._indicatorHidden) {
            return;
        }

        if (hide) {
            this._hideIndicator();
        } else {
            this._restoreIndicator();
        }
        this._indicatorHidden = hide;
    }

    _hideIndicator() {
        this._origShow = this._indicator.show;
        this._indicator.show = () => {};
        this._indicator.hide();

        if (this._indicator.container) {
            this._origContainerShow = this._indicator.container.show;
            this._indicator.container.show = () => {};
            this._indicator.container.hide();
        }

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

        // 独占模式下持续把激活输入源收敛回微澜，避免用户误切换到系统兜底源
        this._manager = this._indicator._inputSourceManager;
        if (this._manager) {
            this._activateWaylanIfAvailable();
            this._sourcesChangedId = this._manager.connect('sources-changed', () => {
                this._activateWaylanIfAvailable();
            });
        }
    }

    _restoreIndicator() {
        if (this._manager && this._sourcesChangedId) {
            this._manager.disconnect(this._sourcesChangedId);
            this._sourcesChangedId = 0;
            this._manager = null;
        }

        if (this._visibleWatchId) {
            this._indicator.disconnect(this._visibleWatchId);
            this._visibleWatchId = 0;
        }
        if (this._containerVisibleWatchId && this._indicator.container) {
            this._indicator.container.disconnect(this._containerVisibleWatchId);
            this._containerVisibleWatchId = 0;
        }
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
    }

    _setupConfigWatcher() {
        const configDir = GLib.get_user_config_dir();
        const waylanDir = GLib.build_filenamev([configDir, 'waylan']);
        const dirFile = Gio.File.new_for_path(waylanDir);

        try {
            // 目录级监听：可靠捕获文件内容修改、新创建以及原子 rename 替换事件
            this._configMonitor = dirFile.monitor_directory(Gio.FileMonitorFlags.NONE, null);
            this._configMonitorId = this._configMonitor.connect('changed', (mon, f, of, eventType) => {
                const basename = f ? f.get_basename() : '';
                if (basename && basename.startsWith('config.toml')) {
                    if (this._configDebounceTimer) {
                        GLib.source_remove(this._configDebounceTimer);
                    }
                    this._configDebounceTimer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 150, () => {
                        this._configDebounceTimer = 0;
                        this._applyCurrentConfig();
                        return GLib.SOURCE_REMOVE;
                    });
                }
            });
        } catch (e) {
            console.error('[WaylanExtension] 创建配置文件目录监听失败:', e);
        }
    }

    _applyCurrentConfig() {
        const config = readWaylanConfig();
        this._currentConfig = config;

        // 职责一：仅在微澜独占输入源时隐藏顶栏原生指示器
        this._applyIndicatorVisibility(config.hideIndicator);

        // 职责二：按候选框模式决定是否接管 IBus 候选弹窗
        if (config.windowMode === 'native') {
            // 桌面系统原生模式：卸载微澜自定义视图，仅保留上述指示器隐藏职责
            this._unmountWaylanView();
        } else {
            // 微澜现代专属模式：接管候选弹窗或热更新样式
            if (!this._waylanView) {
                this._mountWaylanView(config);
            } else {
                this._waylanView.applyConfig(config);
            }
        }
    }

    _mountWaylanView(config) {
        try {
            const ibusManager = IBusManager.getIBusManager();
            if (!ibusManager || !ibusManager._candidatePopup) {
                return;
            }
            const popup = ibusManager._candidatePopup;
            this._candidatePopup = popup;

            const getPs = () => popup._panelService || ibusManager._panelService;
            this._waylanView = new WaylanCandidateView(getPs, config);

            // 保存原始子控件并将弹窗内容替换为微澜胶囊组件
            this._origPopupChild = popup.bin.get_child();
            popup.bin.set_child(this._waylanView.actor);

            const area = popup._candidateArea;
            if (area) {
                this._candidateArea = area;
                this._origSetCandidates = area.setCandidates;
                const view = this._waylanView;

                area.setCandidates = function(indexes, candidates, cursorPosition, cursorVisible) {
                    view.updateCandidates(indexes, candidates, cursorPosition, cursorVisible);
                };

                this._origUpdateButtons = area.updateButtons;
                area.updateButtons = function(wrapsAround, page, nPages) {
                    view.updatePages(page, nPages);
                };

                this._origSetOrientation = area.setOrientation;
                area.setOrientation = function(orientation) {
                    view.setOrientation(orientation);
                };
            }

            // 监听预编辑文字与可见性更新
            if (popup._preeditText) {
                const view = this._waylanView;
                this._preeditNotifyTextId = popup._preeditText.connect('notify::text', () => {
                    view.updatePreedit(popup._preeditText.text, popup._preeditText.visible);
                });
                this._preeditNotifyVisibleId = popup._preeditText.connect('notify::visible', () => {
                    view.updatePreedit(popup._preeditText.text, popup._preeditText.visible);
                });
            }
        } catch (e) {
            console.error('[WaylanExtension] 挂载微澜专属候选框失败:', e);
        }
    }

    _unmountWaylanView() {
        if (!this._candidatePopup) {
            return;
        }

        // 还原弹窗原始子节点
        if (this._origPopupChild) {
            this._candidatePopup.bin.set_child(this._origPopupChild);
            this._origPopupChild = null;
        }

        // 还原 CandidateArea 方法
        if (this._candidateArea) {
            if (this._origSetCandidates) {
                this._candidateArea.setCandidates = this._origSetCandidates;
                this._origSetCandidates = null;
            }
            if (this._origUpdateButtons) {
                this._candidateArea.updateButtons = this._origUpdateButtons;
                this._origUpdateButtons = null;
            }
            if (this._origSetOrientation) {
                this._candidateArea.setOrientation = this._origSetOrientation;
                this._origSetOrientation = null;
            }
            this._candidateArea = null;
        }

        // 断开 preeditText 信号
        if (this._candidatePopup._preeditText) {
            if (this._preeditNotifyTextId) {
                this._candidatePopup._preeditText.disconnect(this._preeditNotifyTextId);
                this._preeditNotifyTextId = 0;
            }
            if (this._preeditNotifyVisibleId) {
                this._candidatePopup._preeditText.disconnect(this._preeditNotifyVisibleId);
                this._preeditNotifyVisibleId = 0;
            }
        }

        // 销毁微澜自定义视图
        if (this._waylanView) {
            this._waylanView.destroy();
            this._waylanView = null;
        }

        this._candidatePopup = null;
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
        if (this._configDebounceTimer) {
            GLib.source_remove(this._configDebounceTimer);
            this._configDebounceTimer = 0;
        }

        if (this._configMonitor && this._configMonitorId) {
            this._configMonitor.disconnect(this._configMonitorId);
            this._configMonitorId = 0;
            this._configMonitor = null;
        }

        // 仅在确实隐藏过指示器时还原，避免把 GNOME 自身的可见性状态强行改写为显示
        if (this._indicatorHidden) {
            if (!this._indicator) {
                this._indicator = Main.panel.statusArea['keyboard'];
            }
            if (this._indicator) {
                this._restoreIndicator();
            }
            this._indicatorHidden = false;
        }

        if (this._focusWindowNotifyId && global.display) {
            global.display.disconnect(this._focusWindowNotifyId);
            this._focusWindowNotifyId = 0;
        }

        try {
            const runtimeDir = GLib.get_user_runtime_dir();
            const filePath = GLib.build_filenamev([runtimeDir, 'waylan_active_window.json']);
            const file = Gio.File.new_for_path(filePath);
            if (file.query_exists(null)) {
                file.delete(null);
            }
        } catch (e) {
            // 忽略临时文件清理异常
        }

        this._indicator = null;
        this._unmountWaylanView();
    }
}
