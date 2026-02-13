import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import St from 'gi://St';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

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

        this._panel.destroy_all_children();

        // Sort toggle button
        const sortLabel = this._sortMode === SORT_RECENT
            ? '\u{1F552} Recent' : '\u{1F524} A\u2013Z';
        const sortBtn = new St.Button({
            label: sortLabel,
            style_class: 'agent-dash-button agent-dash-sort-toggle',
            reactive: true,
        });
        sortBtn.connect('clicked', () => {
            this._sortMode = this._sortMode === SORT_RECENT
                ? SORT_ALPHA : SORT_RECENT;
            this._refresh();
        });
        this._panel.add_child(sortBtn);

        const sessions = data.sessions || [];
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
            reactive: true,
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
        const dot = dots[session.status] || '\u{26AA}';
        const branch = (!session.branch || session.branch === 'main')
            ? '' : ` (${session.branch})`;
        const labelText = `${dot} ${session.project_name}${branch}`;
        const styleClass = styleClasses[session.status] || 'agent-dash-label-grey';

        const label = new St.Label({
            text: labelText,
            style_class: styleClass,
            reactive: true,
        });
        pill.add_child(label);

        const isExpanded = this._expandedSession === session.session_id;

        // Click to expand/collapse
        pill.connect('button-press-event', () => {
            if (isExpanded) {
                this._expandedSession = null;
            } else if (session.input_reason) {
                this._expandedSession = session.session_id;
            }
            this._refresh();
            return true; // consume event
        });

        // Expanded detail
        if (isExpanded && session.input_reason) {
            const reason = session.input_reason;
            if (reason.type === 'permission') {
                const detailText = reason.command
                    ? `${reason.tool}: ${reason.command}`
                    : `${reason.tool}: ${reason.detail || '?'}`;
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
