# Mobile Sidebar Drawer Design

## Problem

The web UI sidebar is always 280px wide, leaving little room for content on phone screens. There are no responsive breakpoints. Users accessing the dashboard from a phone over the network need a mobile-friendly way to navigate between sessions.

## Approach: Drawer overlay with backdrop

On mobile (`max-width: 768px`), the sidebar becomes a slide-in drawer that overlays the main content with a dimmed backdrop. This matches patterns in Slack, Discord, and the Claude app — good for users who frequently switch between many sessions.

Desktop layout is completely unchanged.

## Mobile States

**No session selected (landing):** Sidebar visible full-width, main content hidden. User picks a session to proceed.

**Session selected (content view):** Sidebar off-screen left (`translateX(-100%)`). Main content full-width. Hamburger button visible in top-left.

**Drawer open (overlay):** Sidebar slides in from left as fixed overlay (`z-index: 100`). Semi-transparent backdrop (`z-index: 99`) covers main content. Main content stays in place, dimmed and non-interactive.

## Drawer open/close triggers

- **Open:** Tap hamburger button, or swipe right from left edge (within 20px)
- **Close:** Tap backdrop, select a session, or swipe left on sidebar

## CSS changes

- `@media (max-width: 768px)` breakpoint for all mobile rules
- `#sidebar`: `position: fixed; top: 0; left: 0; width: 80%; max-width: 320px; height: 100%; z-index: 100; transform: translateX(-100%); transition: transform 0.25s ease`
- `.sidebar-open` on `#sidebar`: `transform: translateX(0)`
- `#main`: `width: 100%`
- `#backdrop`: `position: fixed; inset: 0; z-index: 99; background: rgba(0,0,0,0.5); opacity: 0; pointer-events: none; transition: opacity 0.25s ease`
- `#mobile-menu-btn`: hidden on desktop, visible on mobile

## JS changes

- `window.matchMedia('(max-width: 768px)')` listener to track mobile state
- Hamburger button toggles `.sidebar-open` and backdrop visibility
- `selectSession()` closes drawer when in mobile mode
- Edge-swipe handler: touchstart within 20px of left edge, >50px rightward movement opens drawer; leftward swipe on open sidebar closes it
- Backdrop click closes drawer
- On media query change (mobile <-> desktop), clean up mobile classes and restore desktop layout

## Edge cases

- **Orientation/resize change:** matchMedia listener restores correct state
- **Terminal view:** Backdrop z-index sits between terminal and drawer, no conflict
- **Edge swipe vs terminal scroll:** Edge swipe only within 20px of screen edge; terminal content is inset
- **New session modal:** Already uses high z-index fixed positioning, naturally layers above drawer
