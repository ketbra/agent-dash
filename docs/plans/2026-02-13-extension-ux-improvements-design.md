# Extension UX Improvements Design

## Overview
Three improvements to the agent-dash GNOME Shell extension sidebar:
1. Clickier buttons with visual press feedback
2. Always-expanded session pills (no click-to-expand)
3. Chat preview hover popup showing recent conversation from JSONL

## 1. Clickier Buttons with Icons

### Current Problem
Permission action buttons (Allow, Similar, Deny) use text labels that get truncated in the narrow 220px panel. Buttons have no visual feedback when pressed.

### Design
- Replace text labels with GNOME symbolic icons:
  - Allow → `object-select-symbolic` (checkmark), tooltip: "Allow"
  - Similar → `edit-copy-symbolic` (duplicate), tooltip: "Allow similar"
  - Deny → `process-stop-symbolic` (stop/X), tooltip: "Deny"
- Add CSS `:hover` state: brighten background slightly
- Add CSS `:active` state: darken background and shift inward 1px to simulate physical press
- Each color variant (green Allow, yellow Similar, red Deny) gets hover/active states within its color family

### Files Changed
- `extension/stylesheet.css` — Add `:hover` and `:active` rules
- `extension/extension.js` — Replace St.Button text labels with St.Icon children + tooltips

## 2. Always-Expanded Session Pills

### Current Problem
Users must click a session label to expand it and see permission buttons or question details. This extra click is unnecessary friction.

### Design
- Remove `_expandedSession` state variable and toggle logic
- When a session has `input_reason`, always render the detail text and action buttons inline
- Permission detail shows just the tool name inline (e.g., "Bash"); full command visible in hover popup
- Question text always visible inline
- The label button wrapper can remain for future interactions but doesn't toggle expansion

### Files Changed
- `extension/extension.js` — Remove `_expandedSession` tracking, always render details when `input_reason` exists

## 3. Chat Preview Hover Popup

### Trigger
- On `enter-event` of any session pill (all statuses: working, idle, needs_input, ended)
- On `leave-event` from both pill and popup, hide popup
- 100ms delay before hiding so cursor can move from pill to popup without flicker

### Positioning
- Left edge at x = `PANEL_WIDTH` (220px), to the right of the dash panel
- Y position aligned with the hovered pill, clamped so it doesn't overflow screen bottom
- Width: 800px
- Height: up to screen height minus top bar, scrollable if content exceeds it

### Data Source
- Add `jsonl_path` field to `DashSession` serialization (the `Session` struct already has it)
- Extension reads the tail of the JSONL file (~64KB from end, same approach daemon uses)
- Parse last entries, extract content, working backward until ~100 lines of visible text

### Content Extraction
- **Assistant text blocks** → rendered with Pango markup (markdown-to-Pango conversion)
- **Tool use blocks** → compact one-liner: `"> Bash: cargo build --release"` in monospace, blue color
- **User text blocks** → shown with "You:" header, dimmer color
- **Tool results, progress entries** → skipped entirely

### Markdown-to-Pango Conversion (regex-based, no external deps)
- `**bold**` → `<b>bold</b>`
- `*italic*` → `<i>italic</i>`
- `` `inline code` `` → `<tt>code</tt>`
- `# Header` → `<b><span size="large">Header</span></b>`
- Fenced code blocks (```) → `<tt>` block with monospace
- Everything else → plain text (XML-escaped for Pango safety)

### Caching
- Cache parsed Pango content keyed by `(jsonl_path, file_size)`
- On hover, `stat()` the file — if size unchanged, reuse cached content
- Avoids re-reading and re-parsing 64KB of JSONL on every mouse enter

### Popup Widget Structure
- `St.BoxLayout` added to `Main.layoutManager` as chrome (NOT `affectsStruts` — floats over windows)
- Contains `St.ScrollView` with `St.Label` using Pango markup via `clutter_text.set_use_markup(true)`
- Label uses word-wrap mode for long lines within 800px width

### Styling
- Dark background: `rgba(30, 30, 30, 0.95)` matching dash panel
- Subtle 1px lighter left border for visual separation
- Assistant text: `#e0e0e0`
- User text: `#a0a0a0` with "You:" header
- Tool use summaries: `#80b0ff` (blue), monospace
- Code blocks: `#c0c0c0` on slightly darker background

## Files Changed Summary
- `extension/extension.js` — Popup creation/positioning, JSONL reading, markdown-to-Pango converter, hover handlers, caching, icon buttons, remove `_expandedSession`
- `extension/stylesheet.css` — Button `:hover`/`:active` states, popup styles
- `src/session.rs` — Add `jsonl_path: String` to `DashSession` serialization
