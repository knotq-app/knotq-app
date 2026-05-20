## KnotQ 

### Project Overview

KnotQ is a generalized productivity app that combines a structured text editor with a calendar. The name is a temporary placeholder.

### Core Concepts

**Schemes** are the fundamental unit of KnotQ. Each scheme is a plain text file rendered as a hierarchical checkbox/bullet list — similar to Google Docs' `Cmd+Shift+9` list style. The editor is *line-major*: each line is an independently actionable task.

**Folders** organize schemes hierarchically. The sidebar (modeled after the VSCode file explorer) lets users navigate folders and schemes. 

#### Task Types (by date attributes)

Each line/task can have a `start` and/or `end` datetime:

| Start | End | Type | Calendar Item? |
|---|---|---|---|
| ✅ | ✅ | **Event** | Yes |
| ❌ | ✅ | **Assignment** | Yes (due date) |
| ✅ | ❌ | **Reminder** | Yes |
| ❌ | ❌ | **Procedure** | No — planning only |

Dates are set inline within the scheme editor via a small popover date/time picker (doing command s on a task reveals a calendar popover showing `Start 2023/09/24 at 13:47` in .claude-context/scheme-view.png).

#### Nested Tasks & Details

Child bullet points (indented lines under a task) serve as detail/description for their parent. When viewing an event on the calendar, these nested children are shown as the event's body/details.

---

### UI Structure

#### Sidebar (all pages)
- Hierarchical folder + scheme navigator (VSCode-style)
- Color-coded scheme labels that are configurable (e.g., Summer = orange, Chores = yellow, Research = green, Homework = blue, Classes = purple)
- "New" button at the bottom to create a scheme or folder

#### Scheme Editor Page
- Full-width checkbox/bullet list — edit directly inline
- No heavy markup: only `*bold*` and possibly fenced code blocks
- Inline date picker popover to set start/end on any line
- `#channel` syntax to link to another scheme or a specific task within one
- Be able to reorder / move tasks? Or maybe copy paste is enough for that

#### Calendar Page
- Week-view grid (resembling Google Calendar) with time slots on the Y-axis and days on the X-axis
- Events rendered as colored blocks corresponding to their scheme's color
- **Right panel**: Upcoming assignments, reminders, and events listed chronologically; also shows past-due items
- Events on days outside the current week are visible but visually distinguished
- Actions: create, filter, view, and "go to definition" (jump to the originating line in the scheme)
- The right panel is visible on *all* pages, not just Calendar

> .claude-context/union-view.png shows the Calendar page: the week of Sep 24–30 with MATH 15 (purple, Mon/Wed/Fri 11:30–12:30), laundry (yellow, Sun 14:00), Visit friends (red, Sun 18:00), Meet Professor (green, Thu 17:00), and Essay 1 + Math HW due Thu 23:00. The left panel lists upcoming assignments and a "Chores / laundry" reminder.

> .claude-context/scheme-view.png shows the Scheme Editor for the "Research" scheme: a nested checkbox list for a research paper (ELSAN — Ensemble Linear Sum Assignment Network). A date-picker popover is open on the "Meet Professor" task, showing `Start 2023/09/24 at 13:47` with a mini month calendar.
> .claude-context/many.png Shows calendar in the case of many events, highlighting how you should "merge" events into a single bubble ideally if they overlap and it's hard to visually separate. It also shows how all of the different calendar types look.

#### Search Page
- Triggered via `Cmd+K` (Apple Spotlight–style)
- Searches across all schemes, tasks, and calendar items

#### Settings Page
- Standard app settings

#### Nut List (Special Scheme)
- A built-in, non-editable-in-the-normal-sense scheme
- Prompts the user each day to list out what they want to accomplish *tomorrow*
- Encourages intentional daily planning — not a normal scheme, has its own dedicated UI flow

---

### Features & Quality of Life

- **`#channel` linking**: type `#schemeName` or `#schemeName/taskLine` to cross-link
- **Priorities**: ability to set priority levels on tasks (planned)
- **Repeating events**: recurrence rules, similar to Google Calendar
- **Special-cased events**: exceptions/overrides on recurring events
- **Google Calendar integration**: a scheme can be a read/write mirror of a Google Calendar
- **Invite people to events** (stretch goal)
- **Shared schemes**: share a folder or scheme with collaborators
- **Assign people to tasks** (paid tier)
- **Cloud sync** (paid tier)
- **Schedule sharing**: share your free/busy view à la Google Calendar

---

### Backend

- **Language/Framework**: Rust + Axum
- Functionality is primarily CRUD; complexity is expected to be low
- Potential challenge: **synchronization / CRDT** for real-time collaborative editing and multi-device sync

API
- Authentication (for now sign in with something...?)
- Retrieve Schemes, mostly want to retrieve changes
- Search Query
- Send Changes (only really necessary for cloud sync??) Initial version is maybe local only?

### Frontend

| Platform | Stack |
|---|---|
| Web | Svelte + TypeScript |
| Desktop | GPUI / Zed
| iOS / Android | React Native |

---

## Old KnotQ
You can reference old KnotQ in the knotqv1 folder.
You can look at .claude-context for images of knotqv1 as inspiration for the UI.

## GPUI Specific Notes
- You cannot do .on_click without giving the element an id
- Look at this folder for a very concrete example in creating a text editor in GPUI: /home/enigmadux/monocurl/monocurl

## Other
- Never coauthor a message with yourself.
