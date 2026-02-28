// agent-dash web interface
(function () {
  'use strict';

  // --- State ---
  let ws = null;
  let sessions = [];
  let selectedSessionId = null;
  let pendingPermissions = {}; // session_id -> permission info
  let pendingImages = []; // [{mime_type, data, dataUrl}]
  let viewMode = 'terminal';  // 'messages' | 'terminal'
  let terminalInstance = null;
  let fitAddon = null;
  let xtermLoaded = false;
  let xtermLoading = false;
  let terminalFitTimer = null;
  let terminalResizeObserver = null;
  let isMobile = false;

  // --- DOM refs ---
  const sessionList = document.getElementById('session-list');
  const messagesEl = document.getElementById('messages');
  const promptForm = document.getElementById('prompt-form');
  const promptInput = document.getElementById('prompt-input');
  const permBanner = document.getElementById('permission-banner');
  const imagePreviewsEl = document.getElementById('image-previews');
  const suggestionEl = document.getElementById('suggestion');
  const thinkingEl = document.getElementById('thinking-indicator');
  const terminalView = document.getElementById('terminal-view');
  const viewToggleBtn = document.getElementById('view-toggle-btn');
  const collapseBtn = document.getElementById('collapse-btn');
  const sidebar = document.getElementById('sidebar');
  const newSessionBtn = document.getElementById('new-session-btn');
  const newSessionModal = document.getElementById('new-session-modal');
  const modalAgent = document.getElementById('modal-agent');
  const modalCwd = document.getElementById('modal-cwd');
  const modalCwdSuggestions = document.getElementById('modal-cwd-suggestions');
  const modalCancel = document.getElementById('modal-cancel');
  const modalCreate = document.getElementById('modal-create');
  const mobileMenuBtn = document.getElementById('mobile-menu-btn');
  const backdrop = document.getElementById('backdrop');
  const mobileKeys = document.getElementById('mobile-keys');

  // --- WebSocket ---
  function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = function () {
      console.log('WebSocket connected');
      // Re-select session if we had one before reconnect
      if (selectedSessionId) {
        send({ method: 'get_messages', session_id: selectedSessionId, format: 'html', limit: 100 });
        send({ method: 'watch_session', session_id: selectedSessionId, format: 'html' });
        if (viewMode === 'terminal') {
          send({ method: 'watch_terminal', session_id: selectedSessionId });
        }
      }
    };

    ws.onmessage = function (e) {
      const data = JSON.parse(e.data);
      handleEvent(data);
    };

    ws.onclose = function () {
      console.log('WebSocket closed, reconnecting in 2s...');
      setTimeout(connect, 2000);
    };

    ws.onerror = function () {
      ws.close();
    };
  }

  function send(msg) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }

  // --- Event handling ---
  function handleEvent(data) {
    switch (data.event) {
      case 'state_update':
        sessions = data.sessions || [];
        // If selected session is gone, deselect
        if (selectedSessionId && !sessions.find(function(s) { return s.session_id === selectedSessionId; })) {
          selectedSessionId = null;
          messagesEl.innerHTML = '<div id="empty-state">Session ended</div>';
          promptForm.classList.add('hidden');
          permBanner.classList.add('hidden');
        }
        renderSessions();
        updatePermissions();
        updateSuggestion();
        updateThinking();
        break;
      case 'messages':
        renderMessages(data.messages || []);
        break;
      case 'message':
        appendMessage(data.message);
        break;
      case 'permission_pending':
        pendingPermissions[data.session_id] = data;
        updatePermissions();
        renderSessions();
        updateMobileKeys();
        break;
      case 'permission_resolved':
        // Remove resolved permission
        for (const sid in pendingPermissions) {
          if (pendingPermissions[sid].request_id === data.request_id) {
            delete pendingPermissions[sid];
          }
        }
        updatePermissions();
        renderSessions();
        updateMobileKeys();
        break;
      case 'terminal_data':
        if (terminalInstance && data.data) {
          var bytes = base64ToUint8Array(data.data);
          terminalInstance.write(bytes);
        }
        break;
      case 'session_created':
        // Auto-select the newly created session after a short delay
        // (the wrapper needs time to register with the daemon).
        if (data.session_id) {
          var newId = data.session_id;
          var waitAttempts = 0;
          var waitForSession = function () {
            var found = sessions.find(function(s) { return s.session_id === newId; });
            if (found) {
              selectSession(newId);
              setViewMode('terminal');
            } else if (waitAttempts < 20) {
              waitAttempts++;
              setTimeout(waitForSession, 250);
            }
          };
          waitForSession();
        }
        break;
      case 'prompt_sent':
        // Could show a confirmation toast
        break;
      case 'directory_listing':
        renderDirSuggestions(data.path, data.entries);
        break;
      case 'error':
        console.error('Server error:', data.message);
        break;
    }
  }

  // --- Session list ---
  function renderSessions() {
    sessionList.innerHTML = '';
    if (sessions.length === 0) {
      sessionList.innerHTML = '<li style="color:var(--text-dim);padding:8px">No active sessions</li>';
      return;
    }
    sessions.forEach(function (s) {
      const li = document.createElement('li');
      if (s.session_id === selectedSessionId) li.className = 'active';

      const hasPerm = pendingPermissions[s.session_id];
      const subagentBadge = s.subagent_count > 0
        ? ' <span style="color:var(--text-dim);font-size:11px">(+' + s.subagent_count + ')</span>'
        : '';
      const permBadge = hasPerm
        ? ' <span class="badge">!</span>'
        : '';

      var agentLabel = s.agent || s.project_name || '??';
      var abbrev = agentLabel.substring(0, 2).toUpperCase();
      var fullTitle = s.project_name + (s.branch ? ' (' + s.branch + ')' : '');

      li.innerHTML =
        '<div class="session-abbrev" title="' + escapeHtml(fullTitle) + '">' + escapeHtml(abbrev) + '</div>' +
        '<div class="session-project">' + escapeHtml(s.project_name) + subagentBadge + permBadge + '</div>' +
        '<div class="session-branch">' + escapeHtml(s.branch || '') + '</div>' +
        '<div class="session-meta">' +
        '<span class="status-dot ' + (s.status || 'idle') + (s.status === 'working' && !s.active_tool ? ' thinking' : '') + '"></span>' +
        '<span style="font-size:11px;color:var(--text-dim)">' + (s.status === 'working' && !s.active_tool ? 'thinking\u2026' : escapeHtml(s.status || 'idle')) + '</span>' +
        (s.active_tool ? ' <span style="font-size:11px;color:var(--text-dim)">\u2022 ' + escapeHtml(s.active_tool.name) + '</span>' : '') +
        '</div>';

      li.onclick = function () { selectSession(s.session_id); };
      sessionList.appendChild(li);
    });
  }

  function selectSession(id) {
    if (selectedSessionId === id) return;

    // Unwatch previous
    if (selectedSessionId) {
      send({ method: 'unwatch_session', session_id: selectedSessionId });
      if (viewMode === 'terminal') {
        send({ method: 'unwatch_terminal', session_id: selectedSessionId });
      }
    }

    selectedSessionId = id;
    if (isMobile) closeDrawer();
    renderSessions();
    messagesEl.innerHTML = '<div id="empty-state">Loading...</div>';
    if (viewMode !== 'terminal') promptForm.classList.remove('hidden');
    updatePermissions();
    updateSuggestion();
    updateThinking();
    updateMobileKeys();

    // Fetch history then start watching
    send({ method: 'get_messages', session_id: id, format: 'html', limit: 100 });
    send({ method: 'watch_session', session_id: id, format: 'html' });

    // Re-watch terminal for new session if in terminal mode
    if (viewMode === 'terminal') {
      if (terminalInstance) {
        terminalInstance.reset();
        terminalInstance.focus();
        scheduleTerminalFit();
      }
      send({ method: 'watch_terminal', session_id: id });
    } else {
      promptInput.focus();
    }
  }

  function scheduleTerminalFit(delayMs) {
    clearTimeout(terminalFitTimer);
    terminalFitTimer = setTimeout(function () {
      // Session/view changes can alter surrounding layout (banner/prompt visibility).
      // Wait until at least one paint completes before fitting.
      requestAnimationFrame(function () {
        if (viewMode === 'terminal' && terminalInstance && fitAddon && !terminalView.classList.contains('hidden')) {
          fitAddon.fit();
          sendTerminalSize();
        }
      });
    }, delayMs || 0);
  }

  // --- Messages ---
  function renderMessages(msgs) {
    messagesEl.innerHTML = '';
    if (msgs.length === 0) {
      messagesEl.innerHTML = '<div id="empty-state">No messages yet</div>';
      return;
    }
    msgs.forEach(function (m) { appendMessage(m); });
  }

  function appendMessage(msg) {
    // Remove empty state if present
    const empty = messagesEl.querySelector('#empty-state');
    if (empty) empty.remove();

    const div = document.createElement('div');
    div.className = msg.role === 'user' ? 'msg-user' : 'msg-assistant';

    if (typeof msg.content === 'string') {
      div.innerHTML = msg.content;
    } else if (Array.isArray(msg.content)) {
      // Structured content - render blocks
      msg.content.forEach(function (block) {
        if (block.type === 'text') {
          const p = document.createElement('div');
          p.innerHTML = escapeHtml(block.text);
          div.appendChild(p);
        } else if (block.type === 'tool_use') {
          const t = document.createElement('div');
          t.innerHTML = '<strong>' + escapeHtml(block.name) + '</strong>: <code>' + escapeHtml(block.detail || '') + '</code>';
          div.appendChild(t);
        } else if (block.type === 'tool_result') {
          const t = document.createElement('div');
          t.style.color = 'var(--text-dim)';
          t.style.fontSize = '12px';
          t.textContent = '\u21b3 ' + (block.output || '(no output)');
          div.appendChild(t);
        }
      });
    }

    messagesEl.appendChild(div);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  // --- Image paste handling ---
  document.addEventListener('paste', function (e) {
    const items = e.clipboardData && e.clipboardData.items;
    if (!items) return;
    for (let i = 0; i < items.length; i++) {
      if (items[i].type.indexOf('image/') === 0) {
        e.preventDefault();
        const file = items[i].getAsFile();
        if (!file) continue;
        const reader = new FileReader();
        reader.onload = function () {
          const dataUrl = reader.result;
          // dataUrl = "data:image/png;base64,iVBOR..."
          const commaIdx = dataUrl.indexOf(',');
          const meta = dataUrl.substring(0, commaIdx); // "data:image/png;base64"
          const mime_type = meta.split(':')[1].split(';')[0];
          const data = dataUrl.substring(commaIdx + 1);
          pendingImages.push({ mime_type: mime_type, data: data, dataUrl: dataUrl });
          renderImagePreviews();
        };
        reader.readAsDataURL(file);
      }
    }
  });

  function renderImagePreviews() {
    imagePreviewsEl.innerHTML = '';
    if (pendingImages.length === 0) {
      imagePreviewsEl.classList.add('hidden');
      return;
    }
    imagePreviewsEl.classList.remove('hidden');
    pendingImages.forEach(function (img, idx) {
      const item = document.createElement('div');
      item.className = 'image-preview-item';

      const imgEl = document.createElement('img');
      imgEl.src = img.dataUrl;
      item.appendChild(imgEl);

      const removeBtn = document.createElement('button');
      removeBtn.type = 'button';
      removeBtn.className = 'image-remove';
      removeBtn.textContent = '\u00d7';
      removeBtn.onclick = function () {
        pendingImages.splice(idx, 1);
        renderImagePreviews();
      };
      item.appendChild(removeBtn);

      imagePreviewsEl.appendChild(item);
    });
  }

  // --- Prompt injection ---
  promptForm.onsubmit = function (e) {
    e.preventDefault();
    const text = promptInput.value.trim();
    if (!text && pendingImages.length === 0) return;
    if (!selectedSessionId) return;

    const msg = { method: 'send_prompt', session_id: selectedSessionId, text: text };
    if (pendingImages.length > 0) {
      msg.images = pendingImages.map(function (img) {
        return { mime_type: img.mime_type, data: img.data };
      });
    }
    // Clear UI state before sending — ws.send() with large base64
    // payloads can throw, which would skip cleanup.
    promptInput.value = '';
    pendingImages = [];
    renderImagePreviews();
    send(msg);
  };

  // --- Permission UI ---
  function updatePermissions() {
    if (!selectedSessionId || !pendingPermissions[selectedSessionId] || viewMode === 'terminal') {
      permBanner.classList.add('hidden');
      return;
    }

    const perm = pendingPermissions[selectedSessionId];
    permBanner.classList.remove('hidden');

    let html =
      '<div class="perm-info">' +
      '<div class="perm-tool">' + escapeHtml(perm.tool) + '</div>' +
      (function () {
        var detail = perm.detail || '';
        var lastNl = detail.lastIndexOf('\n');
        var cmd, desc;
        if (lastNl === -1) {
          cmd = detail;
          desc = '';
        } else {
          cmd = detail.substring(0, lastNl);
          desc = detail.substring(lastNl + 1);
        }
        var h = '<div class="perm-detail">' + escapeHtml(cmd) + '</div>';
        if (desc) h += '<div class="perm-desc">' + escapeHtml(desc) + '</div>';
        return h;
      })() +
      '</div>' +
      '<button class="btn-allow" onclick="window._permAllow()">Allow</button>' +
      '<button class="btn-deny" onclick="window._permDeny()">Deny</button>';

    if (perm.suggestions && perm.suggestions.length > 0) {
      perm.suggestions.forEach(function (s, i) {
        var label;
        if (s.type === 'toolAlwaysAllow') {
          label = 'Always allow ' + (s.tool || 'this tool');
        } else if (s.type === 'pathAlwaysAllow') {
          label = 'Always allow ' + (s.path || 'this path');
        } else {
          label = 'Allow similar';
        }
        html += ' <button class="btn-suggestion" onclick="window._permSuggest(' + i + ')">' + escapeHtml(label) + '</button>';
      });
    }

    permBanner.innerHTML = html;

    window._permAllow = function () {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'allow'
      });
      refocusActive();
    };

    window._permDeny = function () {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'deny'
      });
      refocusActive();
    };

    window._permSuggest = function (i) {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'allow',
        suggestion: perm.suggestions[i]
      });
      refocusActive();
    };
  }

  // --- Prompt suggestion ---
  function updateSuggestion() {
    if (!selectedSessionId) {
      suggestionEl.classList.add('hidden');
      return;
    }
    var session = sessions.find(function(s) { return s.session_id === selectedSessionId; });
    if (session && session.prompt_suggestion) {
      suggestionEl.textContent = 'Tab: ' + session.prompt_suggestion;
      suggestionEl.classList.remove('hidden');
    } else {
      suggestionEl.classList.add('hidden');
    }
  }

  // --- Thinking indicator ---
  function updateThinking() {
    if (!selectedSessionId || viewMode === 'terminal') {
      thinkingEl.classList.add('hidden');
      return;
    }
    var session = sessions.find(function(s) { return s.session_id === selectedSessionId; });
    if (session && session.thinking_text) {
      thinkingEl.innerHTML = '<span class="thinking-dot"></span> ' + escapeHtml(session.thinking_text);
      thinkingEl.classList.remove('hidden');
    } else {
      var wasVisible = !thinkingEl.classList.contains('hidden');
      thinkingEl.classList.add('hidden');
      if (wasVisible) messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  }

  suggestionEl.onclick = function () {
    var session = sessions.find(function(s) { return s.session_id === selectedSessionId; });
    if (session && session.prompt_suggestion) {
      promptInput.value = session.prompt_suggestion;
      promptInput.focus();
      suggestionEl.classList.add('hidden');
    }
  };

  promptInput.addEventListener('keydown', function (e) {
    if (e.key === 'Tab' && !suggestionEl.classList.contains('hidden')) {
      e.preventDefault();
      var session = sessions.find(function(s) { return s.session_id === selectedSessionId; });
      if (session && session.prompt_suggestion) {
        promptInput.value = session.prompt_suggestion;
        suggestionEl.classList.add('hidden');
      }
    }
  });

  // --- Terminal viewer ---
  async function loadXterm() {
    if (xtermLoaded || xtermLoading) return;
    xtermLoading = true;
    try {
      var xtermMod = await import('https://cdn.jsdelivr.net/npm/@xterm/xterm@6.0.0/+esm');
      var fitMod = await import('https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0.11.0/+esm');
      var webglMod = await import('https://cdn.jsdelivr.net/npm/@xterm/addon-webgl@0.19.0/+esm');
      fitAddon = new fitMod.FitAddon();
      terminalInstance = new xtermMod.Terminal({
        disableStdin: false,
        convertEol: false,
        scrollback: 5000,
        fontSize: 13,
        customGlyphs: true,
        cursorBlink: false,
        cursorInactiveStyle: 'none',
        fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
        theme: {
          background: '#1a1b26',
          foreground: '#c0caf5',
          cursor: '#565f89',
          selectionBackground: '#33467c',
          black: '#15161e',
          red: '#f7768e',
          green: '#9ece6a',
          yellow: '#e0af68',
          blue: '#7aa2f7',
          magenta: '#bb9af7',
          cyan: '#7dcfff',
          white: '#a9b1d6',
          brightBlack: '#414868',
          brightRed: '#f7768e',
          brightGreen: '#9ece6a',
          brightYellow: '#e0af68',
          brightBlue: '#7aa2f7',
          brightMagenta: '#bb9af7',
          brightCyan: '#7dcfff',
          brightWhite: '#c0caf5'
        }
      });
      terminalInstance.loadAddon(fitAddon);
      terminalInstance.open(terminalView);
      terminalInstance.loadAddon(new webglMod.WebglAddon());

      if (!terminalResizeObserver && typeof ResizeObserver !== 'undefined') {
        terminalResizeObserver = new ResizeObserver(function () {
          scheduleTerminalFit();
        });
        terminalResizeObserver.observe(terminalView);
      }
      if (document.fonts && document.fonts.ready) {
        document.fonts.ready.then(function () {
          scheduleTerminalFit();
        });
      }

      // Forward terminal input to the daemon as raw bytes.
      terminalInstance.onData(function (data) {
        if (selectedSessionId) {
          send({ method: 'terminal_input', session_id: selectedSessionId, data: btoa(data) });
        }
      });

      // Prevent the browser from consuming keys we need (Escape, Ctrl-n, etc.).
      // Registering this handler causes xterm.js to call preventDefault() on the
      // DOM keydown event, so the browser never acts on these keys.
      terminalInstance.attachCustomKeyEventHandler(function (ev) {
        if (ev.type === 'keydown') {
          if (ev.key === 'Escape') return true;
          if (ev.ctrlKey && !ev.altKey && !ev.metaKey) return true;
        }
        return true;
      });

      // Touch scroll: translate finger drags into terminal scrollLines()
      // with momentum/flick support for native-feeling mobile scroll.
      (function () {
        var screen = terminalView.querySelector('.xterm-screen') || terminalView;
        var lastY = null;
        var lastTime = 0;
        var accum = 0;
        var velocity = 0;
        var momentumId = null;
        var SPEED = 2.5;
        var lineHeight = Math.ceil(terminalInstance._core._renderService.dimensions.css.cell.height) || 15;

        screen.addEventListener('touchstart', function (e) {
          if (e.touches.length === 1) {
            if (momentumId) { cancelAnimationFrame(momentumId); momentumId = null; }
            lastY = e.touches[0].clientY;
            lastTime = Date.now();
            accum = 0;
            velocity = 0;
          }
        }, { passive: true });

        screen.addEventListener('touchmove', function (e) {
          if (lastY !== null && e.touches.length === 1) {
            var now = Date.now();
            var y = e.touches[0].clientY;
            var dy = lastY - y;
            var dt = now - lastTime;
            lastY = y;
            lastTime = now;
            if (dt > 0) velocity = dy / dt;
            accum += dy * SPEED;
            var lines = Math.trunc(accum / lineHeight);
            if (lines !== 0) {
              terminalInstance.scrollLines(lines);
              accum -= lines * lineHeight;
            }
            e.preventDefault();
          }
        }, { passive: false });

        screen.addEventListener('touchend', function () {
          lastY = null;
          if (Math.abs(velocity) < 0.3) { accum = 0; return; }
          // Momentum: carry the flick velocity with friction decay.
          var v = velocity * lineHeight * SPEED;
          var mAccum = 0;
          function step() {
            v *= 0.95;
            if (Math.abs(v) < 0.5) return;
            mAccum += v;
            var lines = Math.trunc(mAccum / lineHeight);
            if (lines !== 0) {
              terminalInstance.scrollLines(lines);
              mAccum -= lines * lineHeight;
            }
            momentumId = requestAnimationFrame(step);
          }
          momentumId = requestAnimationFrame(step);
          accum = 0;
        }, { passive: true });
      })();

      xtermLoaded = true;
    } catch (e) {
      console.error('Failed to load xterm.js:', e);
    }
    xtermLoading = false;
  }

  function setViewMode(mode) {
    viewMode = mode;
    if (mode === 'terminal') {
      messagesEl.classList.add('hidden');
      promptForm.classList.add('hidden');
      permBanner.classList.add('hidden');
      thinkingEl.classList.add('hidden');
      terminalView.classList.remove('hidden');
      viewToggleBtn.classList.add('active');
      loadXterm().then(function () {
        // Defer fit until the browser has laid out the now-visible container,
        // otherwise fitAddon reads stale/zero dimensions.
        scheduleTerminalFit();
        if (terminalInstance) terminalInstance.focus();
        if (selectedSessionId) {
          send({ method: 'watch_terminal', session_id: selectedSessionId });
        }
      });
    } else {
      messagesEl.classList.remove('hidden');
      if (selectedSessionId) promptForm.classList.remove('hidden');
      terminalView.classList.add('hidden');
      viewToggleBtn.classList.remove('active');
      updatePermissions();
      if (selectedSessionId) {
        promptInput.focus();
        send({ method: 'unwatch_terminal', session_id: selectedSessionId });
      }
    }
    updateMobileKeys();
  }

  function toggleView() {
    setViewMode(viewMode === 'messages' ? 'terminal' : 'messages');
  }

  viewToggleBtn.onclick = toggleView;

  collapseBtn.onclick = function () {
    sidebar.classList.toggle('collapsed');
    collapseBtn.innerHTML = sidebar.classList.contains('collapsed') ? '&#9654;' : '&#9664;';
    refocusActive();
  };

  // After sidebar transition completes, refit terminal to new container size.
  sidebar.addEventListener('transitionend', function (e) {
    if (e.propertyName === 'width' && viewMode === 'terminal' && fitAddon) {
      fitAddon.fit();
      sendTerminalSize();
    }
  });

  // --- Mobile drawer ---
  function openDrawer() {
    sidebar.classList.add('mobile-open');
    backdrop.classList.add('visible');
  }

  function closeDrawer() {
    sidebar.classList.remove('mobile-open');
    backdrop.classList.remove('visible');
    if (viewMode === 'terminal' && fitAddon) {
      setTimeout(function () { fitAddon.fit(); sendTerminalSize(); }, 260);
    }
  }

  mobileMenuBtn.onclick = openDrawer;
  backdrop.onclick = closeDrawer;

  // Track mobile state via matchMedia.
  var mobileQuery = window.matchMedia('(max-width: 768px)');
  function onMobileChange(e) {
    isMobile = e.matches;
    if (!isMobile) {
      closeDrawer();
    }
    updateMobileKeys();
  }
  mobileQuery.addEventListener('change', onMobileChange);
  onMobileChange(mobileQuery);

  // Edge swipe: swipe right from left edge opens drawer,
  // swipe left on open drawer closes it.
  (function () {
    var startX = null;
    var startY = null;
    var tracking = false;

    document.addEventListener('touchstart', function (e) {
      if (!isMobile) return;
      var touch = e.touches[0];
      // Track touches starting within 50px of the left edge (inward of
      // Android Chrome's ~20px back-gesture zone), or on the open sidebar.
      if (touch.clientX < 50) {
        startX = touch.clientX;
        startY = touch.clientY;
        tracking = true;
      } else if (sidebar.classList.contains('mobile-open') &&
                 (e.target === backdrop || sidebar.contains(e.target))) {
        startX = touch.clientX;
        startY = touch.clientY;
        tracking = true;
      }
    }, { passive: true });

    document.addEventListener('touchmove', function (e) {
      if (!tracking || startX === null) return;
      var dx = e.touches[0].clientX - startX;
      var dy = e.touches[0].clientY - startY;
      // Ignore if more vertical than horizontal.
      if (Math.abs(dy) > Math.abs(dx)) {
        tracking = false;
        return;
      }
    }, { passive: true });

    document.addEventListener('touchend', function (e) {
      if (!tracking || startX === null) return;
      var endX = e.changedTouches[0].clientX;
      var dx = endX - startX;
      startX = null;
      startY = null;
      tracking = false;

      if (dx > 60 && !sidebar.classList.contains('mobile-open')) {
        openDrawer();
      } else if (dx < -60 && sidebar.classList.contains('mobile-open')) {
        closeDrawer();
      }
    }, { passive: true });
  })();

  // --- Mobile arrow key bar ---
  function updateMobileKeys() {
    if (!isMobile || viewMode !== 'terminal' || !selectedSessionId ||
        !pendingPermissions[selectedSessionId]) {
      mobileKeys.classList.add('hidden');
    } else {
      mobileKeys.classList.remove('hidden');
    }
  }

  var keyMap = { up: '\x1b[A', down: '\x1b[B', enter: '\r' };
  mobileKeys.addEventListener('click', function (e) {
    var btn = e.target.closest('button[data-key]');
    if (!btn || !selectedSessionId) return;
    var seq = keyMap[btn.dataset.key];
    if (seq) {
      send({ method: 'terminal_input', session_id: selectedSessionId, data: btoa(seq) });
    }
  });

  function sendTerminalSize() {
    if (!selectedSessionId || !terminalInstance) return;
    send({
      method: 'terminal_resize',
      session_id: selectedSessionId,
      cols: terminalInstance.cols,
      rows: terminalInstance.rows
    });
  }

  // --- New session modal ---
  var dirDebounceTimer = null;
  var highlightedIdx = -1;

  newSessionBtn.onclick = function () {
    newSessionModal.classList.remove('hidden');
    modalCwd.value = '';
    modalCwdSuggestions.classList.add('hidden');
    highlightedIdx = -1;
    modalCwd.focus();
    // Kick off initial listing for home directory
    send({ method: 'list_directory' });
  };

  modalCancel.onclick = closeNewSessionModal;

  newSessionModal.onclick = function (e) {
    if (e.target === newSessionModal) closeNewSessionModal();
  };

  function closeNewSessionModal() {
    newSessionModal.classList.add('hidden');
    modalCwdSuggestions.classList.add('hidden');
    refocusActive();
  }

  modalCreate.onclick = function () {
    var agent = modalAgent.value;
    var cwd = modalCwd.value.trim() || undefined;
    var msg = { method: 'create_session', agent: agent };
    if (cwd) msg.cwd = cwd;
    if (terminalInstance) {
      msg.cols = terminalInstance.cols;
      msg.rows = terminalInstance.rows;
    }
    send(msg);
    closeNewSessionModal();
  };

  // Directory autocomplete
  modalCwd.addEventListener('input', function () {
    clearTimeout(dirDebounceTimer);
    dirDebounceTimer = setTimeout(function () {
      var val = modalCwd.value;
      // Determine parent directory to list
      var parentDir;
      if (val === '' || val === '/') {
        parentDir = val || undefined;
      } else if (val.endsWith('/')) {
        parentDir = val;
      } else {
        var lastSlash = val.lastIndexOf('/');
        parentDir = lastSlash >= 0 ? val.substring(0, lastSlash + 1) : undefined;
      }
      var msg = { method: 'list_directory' };
      if (parentDir) msg.path = parentDir;
      send(msg);
    }, 200);
  });

  modalCwd.addEventListener('keydown', function (e) {
    var items = modalCwdSuggestions.querySelectorAll('li');
    if (items.length === 0 || modalCwdSuggestions.classList.contains('hidden')) {
      if (e.key === 'Escape') {
        closeNewSessionModal();
        e.preventDefault();
      } else if (e.key === 'Enter') {
        modalCreate.click();
        e.preventDefault();
      }
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightedIdx = Math.min(highlightedIdx + 1, items.length - 1);
      updateHighlight(items);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightedIdx = Math.max(highlightedIdx - 1, -1);
      updateHighlight(items);
    } else if (e.key === 'Tab' || e.key === 'Enter') {
      e.preventDefault();
      if (highlightedIdx >= 0 && highlightedIdx < items.length) {
        selectSuggestion(items[highlightedIdx].dataset.path);
      } else if (items.length === 1) {
        selectSuggestion(items[0].dataset.path);
      } else if (e.key === 'Enter') {
        modalCreate.click();
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      modalCwdSuggestions.classList.add('hidden');
      highlightedIdx = -1;
    }
  });

  function updateHighlight(items) {
    for (var i = 0; i < items.length; i++) {
      items[i].classList.toggle('highlighted', i === highlightedIdx);
    }
    if (highlightedIdx >= 0 && items[highlightedIdx]) {
      items[highlightedIdx].scrollIntoView({ block: 'nearest' });
    }
  }

  function selectSuggestion(fullPath) {
    modalCwd.value = fullPath;
    highlightedIdx = -1;
    modalCwdSuggestions.classList.add('hidden');
    modalCwd.focus();
    // Fetch next level
    send({ method: 'list_directory', path: fullPath });
  }

  function renderDirSuggestions(parentPath, entries) {
    modalCwdSuggestions.innerHTML = '';
    highlightedIdx = -1;

    if (!entries || entries.length === 0) {
      modalCwdSuggestions.classList.add('hidden');
      return;
    }

    // Filter by typed prefix
    var val = modalCwd.value;
    var prefix = '';
    if (val && !val.endsWith('/')) {
      var lastSlash = val.lastIndexOf('/');
      prefix = lastSlash >= 0 ? val.substring(lastSlash + 1).toLowerCase() : val.toLowerCase();
    }

    var normalizedParent = parentPath.endsWith('/') ? parentPath : parentPath + '/';
    var filtered = entries.filter(function (name) {
      return !prefix || name.toLowerCase().indexOf(prefix) === 0;
    });

    if (filtered.length === 0) {
      modalCwdSuggestions.classList.add('hidden');
      return;
    }

    filtered.forEach(function (name) {
      var li = document.createElement('li');
      li.textContent = name + '/';
      li.dataset.path = normalizedParent + name + '/';
      li.onclick = function () {
        selectSuggestion(li.dataset.path);
      };
      modalCwdSuggestions.appendChild(li);
    });
    modalCwdSuggestions.classList.remove('hidden');
  }

  // Close modal on Escape when focus is elsewhere
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && !newSessionModal.classList.contains('hidden')) {
      closeNewSessionModal();
    }
  });

  // Refit terminal on window resize.
  window.addEventListener('resize', function () {
    if (viewMode === 'terminal' && fitAddon) {
      scheduleTerminalFit(150);
    }
  });

  function refocusActive() {
    if (viewMode === 'terminal' && terminalInstance) {
      terminalInstance.focus();
    } else {
      promptInput.focus();
    }
  }

  // --- Utilities ---
  function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  function base64ToUint8Array(b64) {
    var raw = atob(b64);
    var arr = new Uint8Array(raw.length);
    for (var i = 0; i < raw.length; i++) {
      arr[i] = raw.charCodeAt(i);
    }
    return arr;
  }

  // --- Init ---
  messagesEl.innerHTML = '<div id="empty-state">Select a session to view</div>';
  setViewMode(viewMode);
  connect();
})();
