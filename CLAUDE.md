## KnotQ

### Overview

KnotQ is a desktop productivity app that integrates a structured text editor with a calendar. Built with Rust and GPUI (Zed's UI framework). All data is stored locally — no cloud, no account required.

Releases target macOS (ARM + Intel), Windows (MSIX + Inno Setup installer), and Linux (x86_64 tarball).

### Architecture

The repo is a Rust workspace with 17 crates under `desktop/`:

| Crate | Purpose |
|---|---|
| `app` | Main GPUI application — views, navigation, popups, title bar |
| `ui` | Reusable GPUI components (DateField, SingleLineEditor, etc.) |
| `model` | Core data structures: Item, Scheme, Workspace, Settings, Recurrence |
| `commands` | Command enum with apply/undo/redo — all mutations go through here |
| `state` | AppState: workspace + ephemeral UI state, undo stack, selections |
| `storage` | Abstract `StorageBackend` trait (async) |
| `storage-json` | Concrete JSON file backend — reads/writes from user's data directory |
| `theme` | Theme definitions (8 themes: 4 dark, 4 light) |
| `editor` | GPUI text editor with formatting, inline date pickers |
| `editor-core` | Editor state machine, line manipulation, indent/dedent logic |
| `notifications` | Native notifications: macOS (UserNotifications), Windows (Win32), Linux (stub) |
| `index` | Full-text search index + channel (#link) index |
| `rrule` | RRULE (RFC 5545) parsing/expansion for recurring events |
| `date-util` | Calendar math and formatting helpers |
| `history` | Workspace snapshot/undo-redo history |
| `import` | iCal parsing + Google Calendar OAuth/sync (feature-gated behind `google`) |
| `fixtures` | Test data fixtures |

### Core Concepts

**Schemes** are the fundamental unit. Each scheme is a document rendered as a hierarchical list. The editor is line-major: each line is independently addressable and can be scheduled.

**Folders** organize schemes in a sidebar tree (VSCode-style file explorer).

#### Line Types (markers)

- **Blank** — plain text or heading (`Cmd+1`)
- **Checkbox** — todo item (`Cmd+2`)
- **Bullet** — bullet point (`Cmd+3`)
- **Numbered** — numbered list item (`Cmd+4`)

#### Task Types (by date attributes)

| Start | End | Type |
|---|---|---|
| ✅ | ✅ | **Event** — time block on calendar |
| ❌ | ✅ | **Assignment** — due date on calendar |
| ✅ | ❌ | **Reminder** — alert point on calendar |
| ❌ | ❌ | **Procedure** — planning only, not on calendar |

Dates set via `Cmd+S` (start), `Cmd+E` (end). Calendar interactions: click → reminder, shift+click → assignment, drag → event.

#### Nested Tasks

Child lines (indented under a task) become the event's description on the calendar.

### Views

- **Calendar** (`View::Union`) — week grid, color-coded events from all schemes, upcoming panel on left
- **Scheme Editor** (`View::Scheme`) — inline list editor with formatting toolbar
- **Daily Queue** (`View::DailyQueue`) — per-day notes, incomplete items carry over
- **Settings** (`View::Settings`) — theme, Google Calendar, notifications
- **Search** (`Cmd+F`) — full-text across all schemes

### Themes

8 themes total:
- **Default mode:** System
- **Dark:** Obsidian, Rosé Piné Moon, Catppuccin Mocha, Tokyo Night
- **Light:** Light, Parchment, Rosé Piné Dawn, Catppuccin Latte

### Data Storage

Schemes are stored as `.knotq` files (markdown-like with `!knotq{key="value"}` attribute blocks). Workspace metadata in `workspace.json`. Daily Queue entries in `daily_queue/YYYY/MM/DD.knotq`.

Platform data directory:
- macOS: `~/Library/Application Support/KnotQ/workspace/`
- Linux: `~/.local/share/knotq/`
- Windows: AppData equivalent

### Google Calendar

Read-only import with OAuth 2.0 (PKCE). Background sync every 5 minutes. Feature-gated behind `google` Cargo feature. Imported calendars appear as read-only schemes.

### Notifications

Native notifications on macOS and Windows with actions (mark done, snooze 10min/1hr). Linux has a stub implementation. Per-task notification offset override, configurable defaults for events vs assignments.

### GPUI Notes

- Elements need an `.id()` before `.on_click()` can be used
- All `Cmd+*` shortcuts also have `Ctrl+*` bindings for Windows/Linux

### Other

- Never coauthor a message with yourself.
