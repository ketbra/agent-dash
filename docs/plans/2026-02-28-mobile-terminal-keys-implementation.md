# Mobile Terminal Arrow Keys Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Show a floating Up/Down/Enter button bar on mobile when a permission prompt is active, auto-hiding when resolved.

**Architecture:** A hidden `div` with three buttons in `index.html`, styled in the mobile media query, toggled visible by JS when `PermissionPending` / `PermissionResolved` events fire for the selected session. Buttons inject escape sequences via the existing `terminal_input` WebSocket message.

**Tech Stack:** Vanilla HTML/CSS/JS, no new dependencies.

---

### Task 1: Add the button bar HTML

**Files:**
- Modify: `crates/agent-dash/web/index.html:27` (inside `#terminal-view`, after the terminal container)

**Step 1: Add the arrow key bar element**

In `index.html`, the `#terminal-view` div is currently empty (xterm.js mounts into it dynamically). Add the button bar as a child so it sits inside the terminal area:

```html
      <div id="terminal-view" class="hidden">
        <div id="mobile-keys" class="hidden">
          <button type="button" data-key="up" title="Up">&#9650;</button>
          <button type="button" data-key="down" title="Down">&#9660;</button>
          <button type="button" data-key="enter" title="Enter">&#9166;</button>
        </div>
      </div>
```

The `data-key` attributes let JS map each button to the right escape sequence without separate IDs.

**Step 2: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 3: Commit**

```
git add crates/agent-dash/web/index.html
git commit -m "feat(web): add mobile arrow key bar HTML"
```

---

### Task 2: Add CSS for the button bar

**Files:**
- Modify: `crates/agent-dash/web/style.css` (add base styles before the media query, add mobile visibility inside the media query)

**Step 1: Add base styles (hidden on desktop)**

Before the `@media (max-width: 768px)` block, add:

```css
#mobile-keys {
  display: none;
}
```

**Step 2: Add mobile styles inside the media query**

Inside the `@media (max-width: 768px)` block, before the closing `}`, add:

```css
  #mobile-keys {
    display: flex;
    justify-content: center;
    gap: 12px;
    padding: 8px;
    position: absolute;
    bottom: 8px;
    left: 0;
    right: 0;
    z-index: 10;
  }

  #mobile-keys button {
    width: 56px;
    height: 44px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: var(--text);
    font-size: 20px;
    cursor: pointer;
    opacity: 0.85;
  }

  #mobile-keys button:active {
    background: var(--accent);
    color: var(--bg);
  }
```

Note: `#mobile-keys` is inside `#terminal-view` which already has `position: relative` from `#main`. The `position: absolute` anchors the bar to the bottom of the terminal view. The `.hidden` class (`display: none !important`) still overrides when no permission is pending.

**Step 3: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 4: Commit**

```
git add crates/agent-dash/web/style.css
git commit -m "feat(web): add mobile arrow key bar CSS"
```

---

### Task 3: Add JS logic to show/hide and send keys

**Files:**
- Modify: `crates/agent-dash/web/app.js`

**Step 1: Add DOM ref**

After the `const backdrop = ...` line (line 41), add:

```javascript
  const mobileKeys = document.getElementById('mobile-keys');
```

**Step 2: Add show/hide helper**

After the `closeDrawer` function and the mobile drawer section (around line 770), add:

```javascript
  // --- Mobile arrow key bar ---
  function updateMobileKeys() {
    if (!isMobile || viewMode !== 'terminal' || !selectedSessionId ||
        !pendingPermissions[selectedSessionId]) {
      mobileKeys.classList.add('hidden');
    } else {
      mobileKeys.classList.remove('hidden');
    }
  }
```

**Step 3: Wire up button clicks**

After the `updateMobileKeys` function, add:

```javascript
  var keyMap = { up: '\x1b[A', down: '\x1b[B', enter: '\r' };
  mobileKeys.addEventListener('click', function (e) {
    var btn = e.target.closest('button[data-key]');
    if (!btn || !selectedSessionId) return;
    var seq = keyMap[btn.dataset.key];
    if (seq) {
      send({ method: 'terminal_input', session_id: selectedSessionId, data: btoa(seq) });
    }
  });
```

**Step 4: Call updateMobileKeys from existing event handlers**

In the `permission_pending` case (line 104-108), add `updateMobileKeys()`:

```javascript
      case 'permission_pending':
        pendingPermissions[data.session_id] = data;
        updatePermissions();
        renderSessions();
        updateMobileKeys();
        break;
```

In the `permission_resolved` case (line 109-118), add `updateMobileKeys()`:

```javascript
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
```

In the `selectSession()` function, after the existing `updateThinking();` call (around line 212), add:

```javascript
    updateMobileKeys();
```

In the `onMobileChange` function, after `closeDrawer()` or at the end, add:

```javascript
    updateMobileKeys();
```

**Step 5: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 6: Commit**

```
git add crates/agent-dash/web/app.js
git commit -m "feat(web): show arrow key bar on mobile permission prompts"
```

---

### Task 4: Manual testing

**No code changes. Verify:**

1. `cargo build` — compiles cleanly
2. Load web UI on phone via `--web-bind 0.0.0.0`
3. **Desktop browser:** No arrow key bar visible, ever
4. **Mobile, no permission pending:** No arrow key bar visible
5. **Mobile, permission pending in terminal view:** Up/Down/Enter bar appears at bottom of terminal
6. **Tap Up/Down:** Navigates the permission prompt options in the terminal
7. **Tap Enter:** Selects the current option, permission resolves, bar disappears
8. **Permission resolved from another client/terminal:** Bar disappears
9. **Switch sessions while bar is showing:** Bar hides if new session has no pending permission
10. **Switch from terminal to messages view:** Bar hides (viewMode check)
