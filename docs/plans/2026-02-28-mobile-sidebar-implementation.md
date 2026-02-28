# Mobile Sidebar Drawer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** On mobile screens (<=768px), the sidebar becomes a slide-in drawer overlay with backdrop, hamburger button, and edge-swipe gesture support.

**Architecture:** CSS media query controls layout differences. A small JS module manages drawer open/close state, backdrop, hamburger button, and edge-swipe detection. Desktop behavior is completely untouched.

**Tech Stack:** Vanilla CSS media queries, vanilla JS touch events, no new dependencies.

---

### Task 1: Add backdrop div and hamburger button to HTML

**Files:**
- Modify: `crates/agent-dash/web/index.html:23` (inside `<main id="main">`, add hamburger as first child)
- Modify: `crates/agent-dash/web/index.html:37` (after `</div><!-- #app -->`, add backdrop)

**Step 1: Add the hamburger button and backdrop elements**

In `index.html`, add the mobile menu button as the first child of `<main id="main">`:

```html
    <main id="main">
      <button id="mobile-menu-btn" type="button" title="Open sessions">&#9776;</button>
      <div id="permission-banner" class="hidden"></div>
```

Add the backdrop element just before the closing `</div>` of `#app`:

```html
      </form>
    </main>
    <div id="backdrop"></div>
  </div>
```

**Step 2: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 3: Commit**

```
git add crates/agent-dash/web/index.html
git commit -m "feat(web): add mobile hamburger button and backdrop elements"
```

---

### Task 2: Add mobile CSS media query

**Files:**
- Modify: `crates/agent-dash/web/style.css` (append at end of file)

**Step 1: Add mobile menu button base styles (hidden on desktop)**

Append to `style.css`, before any media query:

```css
#mobile-menu-btn {
  display: none;
  width: 40px;
  height: 40px;
  border: none;
  background: transparent;
  color: var(--text-dim);
  font-size: 22px;
  cursor: pointer;
  position: absolute;
  top: 8px;
  left: 8px;
  z-index: 10;
  border-radius: 6px;
}

#mobile-menu-btn:hover {
  color: var(--text);
  background: var(--bg-surface);
}

#backdrop {
  display: none;
}
```

**Step 2: Add the mobile media query**

Append the full mobile breakpoint:

```css
@media (max-width: 768px) {
  #mobile-menu-btn {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  #main {
    position: relative;
    width: 100%;
  }

  /* Push content below the hamburger area */
  #messages { padding-top: 52px; }
  #terminal-view { padding-top: 44px; }
  #permission-banner { margin-top: 44px; }

  /* Sidebar becomes a fixed drawer, off-screen by default */
  #sidebar {
    position: fixed;
    top: 0;
    left: 0;
    width: 80%;
    max-width: 320px;
    min-width: 0;
    height: 100%;
    z-index: 200;
    transform: translateX(-100%);
    transition: transform 0.25s ease;
  }

  #sidebar.mobile-open {
    transform: translateX(0);
  }

  /* Disable desktop collapse on mobile */
  #sidebar.collapsed {
    width: 80%;
    max-width: 320px;
    padding: 16px;
  }
  #sidebar.collapsed #sidebar-header { flex-direction: row; gap: 0; }
  #sidebar.collapsed #sidebar-header h2 { display: block; }
  #sidebar.collapsed #sidebar-header > div { flex-direction: row; }
  #sidebar.collapsed #session-list li { padding: 10px 12px; text-align: left; }
  #sidebar.collapsed .session-project,
  #sidebar.collapsed .session-branch,
  #sidebar.collapsed .session-meta { display: flex; }
  #sidebar.collapsed .session-abbrev { display: none; }

  /* Hide the desktop collapse button on mobile */
  #collapse-btn { display: none !important; }

  /* Backdrop overlay */
  #backdrop {
    display: block;
    position: fixed;
    inset: 0;
    z-index: 199;
    background: rgba(0, 0, 0, 0.5);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.25s ease;
  }

  #backdrop.visible {
    opacity: 1;
    pointer-events: auto;
  }
}
```

**Step 3: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 4: Commit**

```
git add crates/agent-dash/web/style.css
git commit -m "feat(web): add mobile sidebar drawer CSS with media query"
```

---

### Task 3: Add mobile drawer JS logic

**Files:**
- Modify: `crates/agent-dash/web/app.js:31` (add DOM ref for new elements)
- Modify: `crates/agent-dash/web/app.js:8` (add mobile state variable)
- Modify: `crates/agent-dash/web/app.js:191-225` (selectSession closes drawer on mobile)
- Modify: `crates/agent-dash/web/app.js:675-687` (after collapse button handler, add mobile drawer code)

**Step 1: Add DOM refs and state**

After `let terminalResizeObserver = null;` (line 17), add:

```javascript
  let isMobile = false;
```

After `const modalCreate = ...` (line 38), add:

```javascript
  const mobileMenuBtn = document.getElementById('mobile-menu-btn');
  const backdrop = document.getElementById('backdrop');
```

**Step 2: Add mobile drawer functions**

After the `sidebar.addEventListener('transitionend', ...)` block (after line 687), add the mobile drawer module:

```javascript
  // --- Mobile drawer ---
  function openDrawer() {
    sidebar.classList.add('mobile-open');
    backdrop.classList.add('visible');
  }

  function closeDrawer() {
    sidebar.classList.remove('mobile-open');
    backdrop.classList.remove('visible');
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
      // Only track touches starting near the left edge, or on the open sidebar.
      if (touch.clientX < 25) {
        startX = touch.clientX;
        startY = touch.clientY;
        tracking = true;
      } else if (sidebar.classList.contains('mobile-open')) {
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
```

**Step 3: Close drawer on session selection in mobile mode**

In the `selectSession()` function, after `selectedSessionId = id;` (line 202), add:

```javascript
    if (isMobile) closeDrawer();
```

**Step 4: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 5: Commit**

```
git add crates/agent-dash/web/app.js
git commit -m "feat(web): add mobile drawer JS with edge-swipe and backdrop"
```

---

### Task 4: Handle terminal refit after drawer close

**Files:**
- Modify: `crates/agent-dash/web/app.js` (inside `closeDrawer` function)

**Step 1: Refit terminal after drawer animation**

The sidebar transition can affect terminal layout on mobile. Update `closeDrawer`:

```javascript
  function closeDrawer() {
    sidebar.classList.remove('mobile-open');
    backdrop.classList.remove('visible');
    if (viewMode === 'terminal' && fitAddon) {
      setTimeout(function () { fitAddon.fit(); sendTerminalSize(); }, 260);
    }
  }
```

**Step 2: Verify build**

Run: `cargo build 2>&1 | tail -3`
Expected: compiles cleanly

**Step 3: Commit**

```
git add crates/agent-dash/web/app.js
git commit -m "fix(web): refit terminal after mobile drawer close"
```

---

### Task 5: Manual testing on mobile

**No code changes. Verify the following scenarios:**

1. `cargo build` — compiles cleanly
2. Load the web UI at `--web-bind 0.0.0.0` from a phone
3. **Landing state:** Sidebar visible full-width, no hamburger visible, session list shown
4. **Select a session:** Sidebar slides out left, main content appears, hamburger visible top-left
5. **Tap hamburger:** Sidebar slides in from left as overlay, backdrop dims content
6. **Tap backdrop:** Sidebar slides back out
7. **Edge swipe right from left edge:** Opens drawer
8. **Swipe left on open drawer:** Closes drawer
9. **Select a session from drawer:** Drawer closes, new session loads
10. **Rotate phone to landscape:** Layout adjusts (still mobile if <=768px)
11. **Desktop browser:** No visual changes at all, sidebar behaves normally
