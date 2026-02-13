import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import St from 'gi://St';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import GSound from 'gi://GSound';

const PANEL_WIDTH = 220;
const REFRESH_INTERVAL_SECONDS = 1;
const STATE_FILE = GLib.build_filenamev([
    GLib.get_user_cache_dir(), 'agent-dash', 'state.json'
]);
const IPC_BASE = GLib.build_filenamev([
    GLib.get_user_cache_dir(), 'agent-dash', 'sessions'
]);

const SORT_RECENT = 'recent';
const SORT_ALPHA = 'alpha';

// Sound event IDs from freedesktop sound theme
const SOUND_FINISHED = 'complete';
const SOUND_NEEDS_INPUT = 'dialog-warning';

export default class AgentDashExtension extends Extension {
    enable() {
        this._panel = new St.BoxLayout({
            vertical: true,
            style_class: 'agent-dash-panel',
            reactive: true,
            x_expand: false,
            y_expand: true,
        });
        this._panel.set_width(PANEL_WIDTH);

        // Position on left edge, below top bar
        const monitor = Main.layoutManager.primaryMonitor;
        const topBarHeight = Main.panel.height || 32;
        this._panel.set_position(0, topBarHeight);
        this._panel.set_height(monitor.height - topBarHeight);

        Main.layoutManager.addChrome(this._panel, {
            affectsStruts: true,
            affectsInputRegion: true,
            trackFullscreen: true,
        });

        this._expandedSession = null;
        this._sortMode = SORT_RECENT;
        this._muted = false;
        this._previousStatuses = {}; // session_id -> status string

        // Initialize GSound
        try {
            this._soundCtx = new GSound.Context();
            this._soundCtx.init(null);
        } catch (e) {
            console.warn('agent-dash: GSound init failed:', e.message);
            this._soundCtx = null;
        }

        this._refresh();
        this._timeoutId = GLib.timeout_add_seconds(
            GLib.PRIORITY_DEFAULT,
            REFRESH_INTERVAL_SECONDS,
            () => {
                this._refresh();
                return GLib.SOURCE_CONTINUE;
            }
        );
    }

    disable() {
        if (this._timeoutId) {
            GLib.source_remove(this._timeoutId);
            this._timeoutId = null;
        }
        if (this._panel) {
            Main.layoutManager.removeChrome(this._panel);
            this._panel.destroy();
            this._panel = null;
        }
        this._expandedSession = null;
        this._soundCtx = null;
        this._previousStatuses = {};
    }

    _playSound(eventId) {
        if (this._muted || !this._soundCtx) return;
        try {
            this._soundCtx.play_simple({'event.id': eventId}, null);
        } catch (e) {
            console.error('agent-dash: sound error:', e.message);
        }
    }

    _checkStatusTransitions(sessions) {
        const currentStatuses = {};
        for (const s of sessions) {
            currentStatuses[s.session_id] = s.status;
            const prev = this._previousStatuses[s.session_id];
            if (prev === undefined) continue; // new session, no sound
            if (prev === s.status) continue; // no change

            if (s.status === 'idle' && (prev === 'working' || prev === 'needs_input')) {
                this._playSound(SOUND_FINISHED);
            } else if (s.status === 'needs_input' && prev !== 'needs_input') {
                this._playSound(SOUND_NEEDS_INPUT);
            }
        }
        this._previousStatuses = currentStatuses;
    }

    _refresh() {
        if (!this._panel) return;

        let data;
        try {
            const [ok, contents] = GLib.file_get_contents(STATE_FILE);
            if (!ok) return;
            const decoder = new TextDecoder();
            data = JSON.parse(decoder.decode(contents));
        } catch (e) {
            return; // File doesn't exist yet or is being written
        }

        const sessions = data.sessions || [];

        // Check for status transitions and play sounds
        this._checkStatusTransitions(sessions);

        this._panel.destroy_all_children();

        // Top toolbar: sort toggle + mute toggle
        const toolbar = new St.BoxLayout({vertical: false});

        const sortLabel = this._sortMode === SORT_RECENT
            ? '\u{1F552} Recent' : '\u{1F524} A\u2013Z';
        const sortBtn = new St.Button({
            label: sortLabel,
            style_class: 'agent-dash-button agent-dash-sort-toggle',
            reactive: true,
            x_expand: true,
        });
        sortBtn.connect('clicked', () => {
            this._sortMode = this._sortMode === SORT_RECENT
                ? SORT_ALPHA : SORT_RECENT;
            this._refresh();
        });
        toolbar.add_child(sortBtn);

        const muteLabel = this._muted ? '\u{1F507}' : '\u{1F50A}';
        const muteBtn = new St.Button({
            label: muteLabel,
            style_class: 'agent-dash-button agent-dash-mute-toggle',
            reactive: true,
        });
        muteBtn.connect('clicked', () => {
            this._muted = !this._muted;
            this._refresh();
        });
        toolbar.add_child(muteBtn);

        this._panel.add_child(toolbar);

        if (sessions.length === 0) {
            const empty = new St.Label({
                text: 'No active Claude sessions',
                style_class: 'agent-dash-empty',
            });
            this._panel.add_child(empty);
            return;
        }

        // Sort sessions
        const sorted = [...sessions];
        if (this._sortMode === SORT_ALPHA) {
            sorted.sort((a, b) => {
                const nameA = a.project_name.toLowerCase();
                const nameB = b.project_name.toLowerCase();
                if (nameA !== nameB) return nameA < nameB ? -1 : 1;
                const branchA = (a.branch || '').toLowerCase();
                const branchB = (b.branch || '').toLowerCase();
                return branchA < branchB ? -1 : branchA > branchB ? 1 : 0;
            });
        } else {
            // Recent: most recently changed status first
            sorted.sort((a, b) =>
                (b.last_status_change || 0) - (a.last_status_change || 0)
            );
        }

        for (const session of sorted) {
            this._addSessionPill(session);
        }
    }

    _addSessionPill(session) {
        const pill = new St.BoxLayout({
            vertical: true,
            style_class: 'agent-dash-pill',
        });

        // Status dot + label
        const dots = {
            needs_input: '\u{1F534}',
            working: '\u{1F7E1}',
            idle: '\u{1F7E2}',
            ended: '\u{26AA}',
        };
        const styleClasses = {
            needs_input: 'agent-dash-label-red',
            working: 'agent-dash-label-yellow',
            idle: 'agent-dash-label-green',
            ended: 'agent-dash-label-grey',
        };
        const branch = (!session.branch || session.branch === 'main')
            ? '' : ` (${session.branch})`;
        const styleClass = styleClasses[session.status] || 'agent-dash-label-grey';

        // Header row: tool icon (or emoji dot) + project name
        const headerRow = new St.BoxLayout({vertical: false});

        if (session.active_tool && session.status === 'working') {
            const icon = new St.Icon({
                icon_name: session.active_tool.icon,
                icon_size: 14,
                style_class: 'agent-dash-tool-icon',
                reactive: true,
            });

            const tooltipText = session.active_tool.detail
                ? (session.active_tool.detail.length > 80
                    ? session.active_tool.detail.slice(0, 77) + '...'
                    : session.active_tool.detail)
                : '';
            const tooltip = new St.Label({
                text: tooltipText,
                style_class: 'agent-dash-tooltip',
                visible: false,
            });
            icon.connect('enter-event', () => { tooltip.visible = true; });
            icon.connect('leave-event', () => { tooltip.visible = false; });

            headerRow.add_child(icon);
            headerRow.add_child(tooltip);
        }

        const labelText = (session.active_tool && session.status === 'working')
            ? `${session.project_name}${branch}`
            : `${dots[session.status] || '\u{26AA}'} ${session.project_name}${branch}`;

        // Clickable label row for expand/collapse
        const labelBtn = new St.Button({
            style_class: styleClass,
            reactive: true,
            x_expand: true,
        });
        labelBtn.set_child(new St.Label({text: labelText}));
        headerRow.add_child(labelBtn);
        pill.add_child(headerRow);

        const isExpanded = this._expandedSession === session.session_id;

        labelBtn.connect('clicked', () => {
            if (isExpanded) {
                this._expandedSession = null;
            } else if (session.input_reason) {
                this._expandedSession = session.session_id;
            }
            this._refresh();
        });

        // Expanded detail
        if (isExpanded && session.input_reason) {
            const reason = session.input_reason;
            if (reason.type === 'permission') {
                let detailText;
                if (reason.command) {
                    // Truncate long commands
                    const cmd = reason.command.length > 80
                        ? reason.command.slice(0, 77) + '...' : reason.command;
                    detailText = `${reason.tool}: ${cmd}`;
                } else {
                    detailText = `${reason.tool}`;
                }
                const detail = new St.Label({
                    text: detailText,
                    style_class: 'agent-dash-detail',
                });
                pill.add_child(detail);

                const buttonBox = new St.BoxLayout({vertical: false});

                const allowBtn = new St.Button({
                    label: 'Allow',
                    style_class: 'agent-dash-button agent-dash-allow',
                });
                allowBtn.connect('clicked', () => {
                    this._writePermissionResponse(session.session_id, 'allow', null);
                    this._expandedSession = null;
                    this._refresh();
                });

                const similarBtn = new St.Button({
                    label: 'Similar',
                    style_class: 'agent-dash-button agent-dash-similar',
                });
                similarBtn.connect('clicked', () => {
                    this._writePermissionResponse(session.session_id, 'allow_similar', null);
                    this._expandedSession = null;
                    this._refresh();
                });

                const denyBtn = new St.Button({
                    label: 'Deny',
                    style_class: 'agent-dash-button agent-dash-deny',
                });
                denyBtn.connect('clicked', () => {
                    this._writePermissionResponse(session.session_id, 'deny',
                        'Denied from dashboard');
                    this._expandedSession = null;
                    this._refresh();
                });

                buttonBox.add_child(allowBtn);
                buttonBox.add_child(similarBtn);
                buttonBox.add_child(denyBtn);
                pill.add_child(buttonBox);
            } else if (reason.type === 'question') {
                const questionLabel = new St.Label({
                    text: reason.text || 'Agent has a question',
                    style_class: 'agent-dash-detail',
                });
                pill.add_child(questionLabel);
            }
        }

        this._panel.add_child(pill);
    }

    _writePermissionResponse(sessionId, behavior, message) {
        try {
            const dir = GLib.build_filenamev([IPC_BASE, sessionId]);
            GLib.mkdir_with_parents(dir, 0o755);

            const response = {decision: {behavior}};
            if (message) response.decision.message = message;

            const path = GLib.build_filenamev([dir, 'permission-response.json']);
            const tmpPath = GLib.build_filenamev([dir, 'permission-response.tmp']);

            const json = JSON.stringify(response);
            GLib.file_set_contents(tmpPath, json);

            const tmpFile = Gio.File.new_for_path(tmpPath);
            const destFile = Gio.File.new_for_path(path);
            tmpFile.move(destFile, Gio.FileCopyFlags.OVERWRITE, null, null);
        } catch (e) {
            console.error('agent-dash: error writing permission response:', e);
        }
    }
}
