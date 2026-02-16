import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import St from 'gi://St';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import GSound from 'gi://GSound';
import Pango from 'gi://Pango';

const PANEL_WIDTH = 220;
const CHAT_POPUP_WIDTH = 800;
const POPUP_HIDE_DELAY_MS = 150;
const SOCKET_PATH = GLib.build_filenamev([
    GLib.get_user_cache_dir(), 'agent-dash', 'daemon.sock'
]);
const RECONNECT_MAX_DELAY = 30;
const CHAT_MESSAGE_LIMIT = 100;

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

        this._sortMode = SORT_RECENT;
        this._muted = false;
        this._previousStatuses = {}; // session_id -> status string
        this._chatCache = {}; // cacheKey -> pango markup string
        this._popupHideTimeout = null;

        // Chat preview popup (floating, doesn't affect struts)
        this._chatLabel = new St.Label({
            style_class: 'agent-dash-chat-label',
        });
        this._chatLabel.clutter_text.set_use_markup(true);
        this._chatLabel.clutter_text.set_line_wrap(true);
        this._chatLabel.clutter_text.set_line_wrap_mode(Pango.WrapMode.WORD_CHAR);
        this._chatLabel.clutter_text.set_ellipsize(Pango.EllipsizeMode.NONE);

        const scrollContent = new St.BoxLayout({
            vertical: true,
            x_expand: true,
        });
        scrollContent.add_child(this._chatLabel);

        const scrollView = new St.ScrollView({
            style_class: 'agent-dash-chat-scroll',
            x_expand: true,
            y_expand: true,
            overlay_scrollbars: true,
        });
        scrollView.set_child(scrollContent);

        this._chatPopup = new St.BoxLayout({
            vertical: true,
            style_class: 'agent-dash-chat-popup',
            reactive: true,
            visible: false,
        });
        this._chatPopup.set_width(CHAT_POPUP_WIDTH);
        this._chatPopup.add_child(scrollView);

        Main.layoutManager.addChrome(this._chatPopup, {
            affectsStruts: false,
            affectsInputRegion: true,
            trackFullscreen: true,
        });

        // Keep popup visible when hovering over it
        this._chatPopup.connect('enter-event', () => {
            this._cancelPopupHide();
        });
        this._chatPopup.connect('leave-event', () => {
            this._schedulePopupHide();
        });

        // Initialize GSound
        try {
            this._soundCtx = new GSound.Context();
            this._soundCtx.init(null);
        } catch (e) {
            console.warn('agent-dash: GSound init failed:', e.message);
            this._soundCtx = null;
        }

        // Daemon connection state
        this._sessions = [];
        this._pendingPermissions = {}; // request_id -> {session_id, tool, detail}
        this._daemonOnline = false;
        this._reconnectDelay = 1;
        this._connection = null;
        this._inputStream = null;
        this._cancellable = null;
        this._reconnectTimeoutId = null;

        this._updateUI(); // shows "daemon offline" initially
        this._connectToDaemon();
    }

    disable() {
        this._disconnectFromDaemon();
        this._cancelPopupHide();
        if (this._chatPopup) {
            Main.layoutManager.removeChrome(this._chatPopup);
            this._chatPopup.destroy();
            this._chatPopup = null;
            this._chatLabel = null;
        }
        if (this._panel) {
            Main.layoutManager.removeChrome(this._panel);
            this._panel.destroy();
            this._panel = null;
        }
        this._soundCtx = null;
        this._previousStatuses = {};
        this._chatCache = {};
    }

    // --- Socket connection management ---

    _connectToDaemon() {
        try {
            const address = new Gio.UnixSocketAddress({path: SOCKET_PATH});
            const client = new Gio.SocketClient();
            client.connect_async(address, null, (obj, result) => {
                try {
                    this._connection = obj.connect_finish(result);
                    this._daemonOnline = true;
                    this._reconnectDelay = 1;

                    // Send subscribe request
                    const output = new Gio.DataOutputStream({
                        base_stream: this._connection.get_output_stream(),
                    });
                    output.put_string('{"method":"subscribe"}\n', null);

                    // Set up async line reader
                    this._cancellable = new Gio.Cancellable();
                    this._inputStream = new Gio.DataInputStream({
                        base_stream: this._connection.get_input_stream(),
                    });
                    this._readNextLine();
                } catch (e) {
                    console.warn('agent-dash: connect failed:', e.message);
                    this._scheduleReconnect();
                }
            });
        } catch (e) {
            console.warn('agent-dash: socket error:', e.message);
            this._scheduleReconnect();
        }
    }

    _readNextLine() {
        if (!this._inputStream || !this._cancellable) return;
        this._inputStream.read_line_async(
            GLib.PRIORITY_DEFAULT,
            this._cancellable,
            (stream, res) => {
                try {
                    const [line] = stream.read_line_finish_utf8(res);
                    if (line === null) {
                        this._handleDisconnect();
                        return;
                    }
                    this._handleEvent(JSON.parse(line));
                    this._readNextLine();
                } catch (e) {
                    if (!this._cancellable?.is_cancelled()) {
                        console.warn('agent-dash: read error:', e.message);
                        this._handleDisconnect();
                    }
                }
            }
        );
    }

    _handleDisconnect() {
        this._closeConnection();
        this._daemonOnline = false;
        this._sessions = [];
        this._pendingPermissions = {};
        this._updateUI();
        this._scheduleReconnect();
    }

    _closeConnection() {
        if (this._cancellable) {
            this._cancellable.cancel();
            this._cancellable = null;
        }
        this._inputStream = null;
        if (this._connection) {
            try { this._connection.close(null); } catch {}
            this._connection = null;
        }
    }

    _scheduleReconnect() {
        if (this._reconnectTimeoutId) return;
        this._reconnectTimeoutId = GLib.timeout_add_seconds(
            GLib.PRIORITY_DEFAULT,
            this._reconnectDelay,
            () => {
                this._reconnectTimeoutId = null;
                this._reconnectDelay = Math.min(
                    this._reconnectDelay * 2, RECONNECT_MAX_DELAY
                );
                this._connectToDaemon();
                return GLib.SOURCE_REMOVE;
            }
        );
    }

    _disconnectFromDaemon() {
        if (this._reconnectTimeoutId) {
            GLib.source_remove(this._reconnectTimeoutId);
            this._reconnectTimeoutId = null;
        }
        this._closeConnection();
    }

    // --- Event handling ---

    _handleEvent(event) {
        switch (event.event) {
            case 'state_update':
                this._onStateUpdate(event.sessions || []);
                break;
            case 'permission_pending':
                this._pendingPermissions[event.request_id] = {
                    session_id: event.session_id,
                    tool: event.tool,
                    detail: event.detail,
                    suggestions: event.suggestions || [],
                };
                break;
            case 'permission_resolved':
                delete this._pendingPermissions[event.request_id];
                break;
        }
    }

    _onStateUpdate(sessions) {
        this._checkStatusTransitions(sessions);
        this._sessions = sessions;
        this._updateUI();
    }

    // --- Short-lived request helper ---

    _sendRequest(request, callback) {
        try {
            const address = new Gio.UnixSocketAddress({path: SOCKET_PATH});
            const client = new Gio.SocketClient();
            client.connect_async(address, null, (obj, result) => {
                let conn;
                try {
                    conn = obj.connect_finish(result);
                    const output = new Gio.DataOutputStream({
                        base_stream: conn.get_output_stream(),
                    });
                    output.put_string(JSON.stringify(request) + '\n', null);

                    if (callback) {
                        const input = new Gio.DataInputStream({
                            base_stream: conn.get_input_stream(),
                        });
                        input.read_line_async(
                            GLib.PRIORITY_DEFAULT, null,
                            (stream, readResult) => {
                                try {
                                    const [line] = stream.read_line_finish_utf8(readResult);
                                    callback(line ? JSON.parse(line) : null, null);
                                } catch (e) {
                                    callback(null, e);
                                } finally {
                                    try { conn.close(null); } catch {}
                                }
                            }
                        );
                    } else {
                        try { conn.close(null); } catch {}
                    }
                } catch (e) {
                    if (conn) try { conn.close(null); } catch {}
                    if (callback) callback(null, e);
                }
            });
        } catch (e) {
            if (callback) callback(null, e);
        }
    }

    // --- Permission response ---

    _sendPermissionResponse(sessionId, decision, suggestion) {
        const entry = Object.entries(this._pendingPermissions)
            .find(([_, p]) => p.session_id === sessionId);
        if (!entry) {
            console.warn('agent-dash: no pending permission for session', sessionId);
            return;
        }
        const [requestId] = entry;
        const request = {
            method: 'permission_response',
            request_id: requestId,
            session_id: sessionId,
            decision: decision,
        };
        if (suggestion != null) {
            request.suggestion = suggestion;
        }
        this._sendRequest(request);
        delete this._pendingPermissions[requestId];
    }

    // --- Chat messages ---

    _fetchMessages(sessionId, callback) {
        this._sendRequest({
            method: 'get_messages',
            session_id: sessionId,
            format: 'markdown',
            limit: CHAT_MESSAGE_LIMIT,
        }, (response, error) => {
            if (error || !response || response.event === 'error') {
                callback(null, error || new Error(response?.message || 'unknown'));
                return;
            }
            callback(response.messages || [], null);
        });
    }

    _renderMessages(messages) {
        const rendered = [];
        for (const msg of messages) {
            const content = typeof msg.content === 'string'
                ? msg.content : '';
            if (!content.trim()) continue;

            if (msg.role === 'user') {
                const escaped = this._xmlEscape(content.trim());
                rendered.push(`<span foreground="#a0a0a0"><b>You:</b> ${escaped}</span>`);
            } else if (msg.role === 'assistant') {
                const pango = this._markdownToPango(content);
                rendered.push(`<span foreground="#e0e0e0">${pango}</span>`);
            }
        }
        return rendered.join('\n\n') || '<span foreground="#808080">(no messages yet)</span>';
    }

    // --- Sound ---

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

    // --- UI rendering ---

    _updateUI() {
        if (!this._panel) return;

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
            this._updateUI();
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
            this._updateUI();
        });
        toolbar.add_child(muteBtn);

        this._panel.add_child(toolbar);

        if (!this._daemonOnline) {
            const offline = new St.Label({
                text: 'Daemon offline \u2014 reconnecting\u2026',
                style_class: 'agent-dash-empty',
            });
            this._panel.add_child(offline);
            return;
        }

        // Filter out wrapper/unidentified sessions with no project name
        const visible = this._sessions.filter(s => s.project_name);

        if (visible.length === 0) {
            const empty = new St.Label({
                text: 'No active Claude sessions',
                style_class: 'agent-dash-empty',
            });
            this._panel.add_child(empty);
            return;
        }

        // Sort sessions
        const sorted = [...visible];
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

        const label = new St.Label({
            text: labelText,
            style_class: styleClass,
            x_expand: true,
        });
        headerRow.add_child(label);
        pill.add_child(headerRow);

        // Hover events for chat popup
        pill.connect('enter-event', () => {
            this._cancelPopupHide();
            this._showChatPopup(session, pill);
        });
        pill.connect('leave-event', () => {
            this._schedulePopupHide();
        });

        // Always show detail + buttons when input is needed (no expand toggle)
        if (session.input_reason) {
            const reason = session.input_reason;
            if (reason.type === 'permission') {
                const detail = new St.Label({
                    text: reason.tool || 'Permission',
                    style_class: 'agent-dash-detail',
                });
                pill.add_child(detail);

                const buttonBox = new St.BoxLayout({vertical: false});

                // Always show Allow and Deny buttons.
                const allowBtn = this._createIconButton(
                    'object-select-symbolic',
                    'agent-dash-button agent-dash-allow',
                    'Allow'
                );
                allowBtn.connect('clicked', () => {
                    this._sendPermissionResponse(session.session_id, 'allow');
                });
                this._addSessionPillButton(buttonBox, allowBtn);

                // Dynamic suggestion buttons from permission_suggestions.
                const permEntry = Object.values(this._pendingPermissions)
                    .find(p => p.session_id === session.session_id);
                const suggestions = permEntry?.suggestions || [];

                for (const suggestion of suggestions) {
                    if (suggestion.type === 'toolAlwaysAllow') {
                        const toolName = suggestion.tool || 'tool';
                        const sugBtn = this._createIconButton(
                            'edit-copy-symbolic',
                            'agent-dash-button agent-dash-similar',
                            `Always allow ${toolName}`
                        );
                        sugBtn.connect('clicked', () => {
                            this._sendPermissionResponse(
                                session.session_id, 'allow', suggestion
                            );
                        });
                        this._addSessionPillButton(buttonBox, sugBtn);
                    }
                }

                const denyBtn = this._createIconButton(
                    'process-stop-symbolic',
                    'agent-dash-button agent-dash-deny',
                    'Deny'
                );
                denyBtn.connect('clicked', () => {
                    this._sendPermissionResponse(session.session_id, 'deny');
                });
                this._addSessionPillButton(buttonBox, denyBtn);

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

    _createIconButton(iconName, styleClass, tooltipText) {
        const btn = new St.Button({
            style_class: styleClass,
            reactive: true,
        });
        const icon = new St.Icon({
            icon_name: iconName,
            icon_size: 16,
        });
        btn.set_child(icon);

        // Tooltip on hover
        const tooltip = new St.Label({
            text: tooltipText,
            style_class: 'agent-dash-tooltip',
            visible: false,
        });
        btn.connect('enter-event', () => { tooltip.visible = true; });
        btn.connect('leave-event', () => { tooltip.visible = false; });

        // Wrap button and tooltip in a container
        const container = new St.BoxLayout({vertical: false});
        container.add_child(btn);
        container.add_child(tooltip);

        // Return the container but attach the click signal to the button
        // Callers connect 'clicked' on the returned object, so return btn
        // but we need the tooltip to appear — let's use a different approach:
        // Return the button, and add tooltip as a sibling in the parent
        btn._tooltip = tooltip;
        return btn;
    }

    _addSessionPillButton(buttonBox, btn) {
        buttonBox.add_child(btn);
        if (btn._tooltip) {
            buttonBox.add_child(btn._tooltip);
        }
    }

    // --- Chat popup methods ---

    _cancelPopupHide() {
        if (this._popupHideTimeout) {
            GLib.source_remove(this._popupHideTimeout);
            this._popupHideTimeout = null;
        }
    }

    _schedulePopupHide() {
        this._cancelPopupHide();
        this._popupHideTimeout = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT,
            POPUP_HIDE_DELAY_MS,
            () => {
                if (this._chatPopup) {
                    this._chatPopup.visible = false;
                }
                this._popupHideTimeout = null;
                return GLib.SOURCE_REMOVE;
            }
        );
    }

    _showChatPopup(session, pill) {
        if (!this._chatPopup || !this._daemonOnline) return;

        const cacheKey = `${session.session_id}_${session.last_status_change || 0}`;
        const cached = this._chatCache[cacheKey];
        if (cached) {
            this._displayChatContent(cached, pill);
            return;
        }

        this._displayChatContent(
            '<span foreground="#808080">Loading\u2026</span>', pill
        );

        this._fetchMessages(session.session_id, (messages, error) => {
            if (error || !messages) return;
            const content = this._renderMessages(messages);
            this._chatCache[cacheKey] = content;
            if (this._chatPopup?.visible) {
                this._displayChatContent(content, pill);
            }
        });
    }

    _displayChatContent(content, pill) {
        try {
            this._chatLabel.clutter_text.set_markup(content);
        } catch (e) {
            // Fallback to plain text if markup fails
            this._chatLabel.set_text(content.replace(/<[^>]+>/g, ''));
        }

        // Position popup to the right of the panel, aligned with pill
        const [pillX, pillY] = pill.get_transformed_position();
        const monitor = Main.layoutManager.primaryMonitor;
        const topBarHeight = Main.panel.height || 32;
        const maxHeight = monitor.height - topBarHeight - 20;
        const popupY = Math.max(topBarHeight, pillY);

        // Let popup size to content, clamped to available screen space
        this._chatPopup.set_height(-1); // reset to natural height
        this._chatPopup.set_position(PANEL_WIDTH, popupY);
        this._chatPopup.visible = true;

        // Clamp height: use preferred height but don't exceed available space
        const [, natHeight] = this._chatPopup.get_preferred_height(CHAT_POPUP_WIDTH);
        const availableHeight = monitor.height - popupY - 20;
        const finalHeight = Math.min(natHeight, availableHeight, maxHeight);
        this._chatPopup.set_height(finalHeight);

        // Scroll to bottom to show most recent content
        const scrollView = this._chatPopup.get_first_child();
        if (scrollView?.vadjustment) {
            const adj = scrollView.vadjustment;
            adj.set_value(adj.upper);
        }
    }

    _xmlEscape(text) {
        return text
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;');
    }

    _markdownToPango(text) {
        let result = this._xmlEscape(text);

        // Fenced code blocks: ```...``` → <tt>...</tt>
        result = result.replace(/```[\s\S]*?```/g, (match) => {
            // Strip the ``` delimiters and optional language tag
            let code = match.slice(3, -3);
            // Remove language tag on first line
            const firstNewline = code.indexOf('\n');
            if (firstNewline !== -1 && firstNewline < 20 && !code.slice(0, firstNewline).includes(' ')) {
                code = code.slice(firstNewline + 1);
            }
            return `<tt>${code.trim()}</tt>`;
        });

        // Headers: # Title → bold + large
        result = result.replace(/^#{1,3}\s+(.+)$/gm, '<b><span size="large">$1</span></b>');

        // Bold: **text** → <b>text</b>
        result = result.replace(/\*\*(.+?)\*\*/g, '<b>$1</b>');

        // Italic: *text* → <i>text</i>  (but not inside bold)
        result = result.replace(/(?<!\*)\*([^*]+?)\*(?!\*)/g, '<i>$1</i>');

        // Inline code: `text` → <tt>text</tt>
        result = result.replace(/`([^`]+?)`/g, '<tt>$1</tt>');

        return result;
    }
}
