// agent-dash web interface
(function () {
  'use strict';

  // --- State ---
  let ws = null;
  let sessions = [];
  let selectedSessionId = null;
  let pendingPermissions = {}; // session_id -> permission info
  let pendingImages = []; // [{mime_type, data, dataUrl}]

  // --- DOM refs ---
  const sessionList = document.getElementById('session-list');
  const messagesEl = document.getElementById('messages');
  const promptForm = document.getElementById('prompt-form');
  const promptInput = document.getElementById('prompt-input');
  const permBanner = document.getElementById('permission-banner');
  const imagePreviewsEl = document.getElementById('image-previews');
  const suggestionEl = document.getElementById('suggestion');

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
        break;
      case 'prompt_sent':
        // Could show a confirmation toast
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

      li.innerHTML =
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
    }

    selectedSessionId = id;
    renderSessions();
    messagesEl.innerHTML = '<div id="empty-state">Loading...</div>';
    promptForm.classList.remove('hidden');
    updatePermissions();

    // Fetch history then start watching
    send({ method: 'get_messages', session_id: id, format: 'html', limit: 100 });
    send({ method: 'watch_session', session_id: id, format: 'html' });
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
    if (!selectedSessionId || !pendingPermissions[selectedSessionId]) {
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
    };

    window._permDeny = function () {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'deny'
      });
    };

    window._permSuggest = function (i) {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'allow',
        suggestion: perm.suggestions[i]
      });
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

  // --- Utilities ---
  function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  // --- Init ---
  messagesEl.innerHTML = '<div id="empty-state">Select a session to view</div>';
  connect();
})();
