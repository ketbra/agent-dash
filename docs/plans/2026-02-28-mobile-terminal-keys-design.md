# Mobile Terminal Arrow Keys Design

## Problem

On mobile, the on-screen keyboard lacks arrow keys. Claude Code's interactive permission prompts require Up/Down arrow navigation and Enter to select. Users on phones cannot interact with these prompts in the terminal view.

## Approach: Permission-triggered floating button bar

When the daemon broadcasts a `PermissionPending` event for the selected session, a floating button bar with Up, Down, and Enter appears at the bottom of the terminal view. The bar auto-hides when `PermissionResolved` fires (regardless of how the permission was resolved — web UI, terminal, or another client). Only shown on mobile (`isMobile` gate).

## Interaction

- **Show:** `PermissionPending` event for the selected session while `isMobile` is true
- **Hide:** `PermissionResolved` event, or session change
- **Up button:** Sends `\x1b[A` (arrow up escape sequence) via `terminal_input`
- **Down button:** Sends `\x1b[B` (arrow down escape sequence) via `terminal_input`
- **Enter button:** Sends `\r` (carriage return) via `terminal_input`

## CSS

- A bar positioned at the bottom of `#terminal-view`, hidden by default
- Three large touch-friendly buttons with adequate spacing
- Only visible inside `@media (max-width: 768px)`
- z-index above the terminal but below the sidebar drawer

## JS

- Reuse existing `terminal_input` WebSocket message path (same as keyboard input)
- Toggle visibility based on `PermissionPending` / `PermissionResolved` events already handled in the WebSocket message handler
- Gated behind `isMobile` — no effect on desktop

## No daemon changes

The `PermissionPending` and `PermissionResolved` events already exist and are broadcast to all web UI subscribers. No protocol or daemon modifications needed.
