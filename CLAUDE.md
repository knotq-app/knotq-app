## KnotQ 

### Project Overview

KnotQ is a generalized productivity app that combines a structured text editor with a calendar. The name is a temporary placeholder.

### Core Concepts

**Schemes** are the fundamental unit of KnotQ. Each scheme is a plain text file rendered as a hierarchical list — similar to Google Docs' `Cmd+Shift+9` list style. The editor is *line-major*: each line is an independently actionable task.

**Folders** organize schemes hierarchically. The sidebar (modeled after the VSCode file explorer) lets users navigate folders and schemes.

#### Line Types (markers)

Each line has a marker type:
- **Blank** — no marker, plain text/heading (`Cmd+1`)
- **Checkbox** — todo item, can be checked off (`Cmd+2`)
- **Bullet** — bullet point (`Cmd+3`)
- **Numbered** — numbered list item (`Cmd+4`)

#### Task Types (by date attributes)

Each line/task can have a `start` and/or `end` datetime:

| Start | End | Type | Calendar Item? |
|---|---|---|---|
| ✅ | ✅ | **Event** | Yes |
| ❌ | ✅ | **Assignment** | Yes (due date) |
| ✅ | ❌ | **Reminder** | Yes |
| ❌ | ❌ | **Procedure** | No — planning only |

Dates are set via:
- **`Cmd+S`** → open/toggle start date picker
- **`Cmd+E`** → open/toggle end date picker
- **Click on calendar** → creates a reminder (start only) on that line
- **Shift+click on calendar** → creates an assignment (end only)
- **Drag on calendar** → creates an event (start + end)

#### Text Formatting

- **`Cmd+B`** → bold
- **`Cmd+I`** → italic
- **`Cmd+J`** → heading

#### Nested Tasks & Details

Child lines (indented under a task) serve as detail/description for their parent. When viewing an event on the calendar, nested children are shown as the event's body/details.

---

### UI Structure

#### Sidebar (all pages)
- Hierarchical folder + scheme navigator (VSCode-style)
- Color-coded scheme labels (configurable per scheme)
- "New" button at the bottom to create a scheme or folder

#### Scheme Editor Page
- Full-width list editor — edit directly inline
- Formatting: bold, italic, heading
- Inline date picker popovers to set start/end on any line
- Right-click context menu for additional actions

#### Calendar Page (View::Union)
- Week-view grid with time slots on Y-axis and days on X-axis
- Events rendered as colored blocks corresponding to their scheme's color
- **Upcoming panel**: Upcoming assignments, reminders, and events; also shows past-due items
- Events on days outside the current week are visible but visually distinguished
- Click any event to jump to its source line in the scheme editor ("go to definition")
- The upcoming panel is visible on all pages

> .claude-context/union-view.png shows the Calendar page.
> .claude-context/scheme-view.png shows the Scheme Editor with date-picker popover open.
> .claude-context/many.png shows calendar with many overlapping events (merged into bubbles).

#### Search (Cmd+F)
- Searches across all schemes, tasks, and calendar items
- Triggered via `Cmd+F`

#### Settings Page
- Standard app settings (theme, Google Calendar connection, etc.)

#### Daily Queue (View::DailyQueue, shown as "Daily")
- A special built-in view — not an ordinary scheme
- Creates a new dated scheme for each day (named "Daily YYYY-MM-DD")
- Incomplete items carry over automatically from the previous day
- Intended for quick notes, brainstorming, and listing tasks to complete that day
- Has its own dedicated UI; users navigate by day

---

### Keyboard Shortcuts (editor)

| Shortcut | Action |
|---|---|
| `Cmd+S` | Toggle start date (open date picker) |
| `Cmd+E` | Toggle end date |
| `Cmd+Shift+S` | Remove start date |
| `Cmd+Shift+E` | Remove end date |
| `Cmd+R` | Toggle repeat rule |
| `Cmd+L` | Toggle status (complete/incomplete) |
| `Cmd+B` | Toggle bold |
| `Cmd+I` | Toggle italic |
| `Cmd+J` | Toggle heading |
| `Cmd+1` | Set line type: blank |
| `Cmd+2` | Set line type: checkbox |
| `Cmd+3` | Set line type: bullet |
| `Cmd+4` | Set line type: numbered |
| `Tab` | Indent line |
| `Shift+Tab` | Unindent line |
| `Cmd+F` | Search |

All `Cmd+*` shortcuts also work with `Ctrl+*` on Windows/Linux.

---

### Features (implemented vs. planned)

**Implemented:**
- Scheme editor with line types, formatting, indentation
- Date picker (start/end), repeating events with rrule
- Calendar view (week grid, event/assignment/reminder/procedure rendering)
- Daily Queue (per-day notes + carryover)
- Google Calendar integration (read-only)
- Search (Cmd+F)
- Notifications (macOS + Windows)
- Multiple themes (dark/light)

**Planned / in progress:**
- `#channel` cross-linking — index is built in code but not yet exposed in the editor UI
- Priorities on tasks
- Cloud sync (paid tier)
- Shared schemes / assign people (paid tier)
- Schedule sharing (free/busy view)

---

### Backend

- **Language/Framework**: Rust + Axum
- Primarily CRUD; complexity expected to be low
- Potential challenge: **synchronization / CRDT** for real-time collaborative editing and multi-device sync
- Initial version is local-only (no cloud)

### Frontend

| Platform | Stack |
|---|---|
| Desktop | GPUI (Zed's UI framework) |
| Web (future) | Svelte + TypeScript |
| Mobile (future) | React Native |

---

## Old KnotQ
You can reference old KnotQ in the knotqv1 folder.
You can look at .claude-context for images of knotqv1 as inspiration for the UI.

## GPUI Specific Notes
- You cannot do .on_click without giving the element an id
- Look at this folder for a very concrete example in creating a text editor in GPUI: /home/enigmadux/monocurl/monocurl

## Other
- Never coauthor a message with yourself.
