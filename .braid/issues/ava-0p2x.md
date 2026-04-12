---
schema_version: 9
id: ava-0p2x
title: add accessibility tree snapshot action to browser tool
priority: P2
status: done
deps:
- ava-i727
tags:
- tool
- browser
owner: null
created_at: 2026-02-11T10:20:24.105157Z
started_at: 2026-03-18T21:32:05.530517Z
completed_at: 2026-03-19T08:37:25.437715Z
---

add a `snapshot` action to the browser tool that returns the page's accessibility tree in a compact, LLM-friendly format with numeric refs for interactive elements.

### why

CSS selectors are brittle and require the agent to guess at page structure. an accessibility tree snapshot gives a structured view of what's on the page — roles, names, states — and lets the agent target elements by ref number instead of selector. this is how playwright MCP and other AI browser tools work.

### design decisions

- **action name**: `snapshot` (added to existing browser tool's action enum)
- **output format**: indented tree, playwright-style ARIA snapshot format
- **ref system**: short numeric refs (`[1]`, `[2]`, ...) assigned only to interactive elements (links, buttons, inputs, etc.)
- **ref targeting**: extend `click` and `type` actions to accept `ref` as an alternative to `selector`
- **filtering**: skip ignored nodes, collapse structural wrappers (generic/group/div with no name), depth limit via CDP parameter

### output format

```
[1] heading "Dashboard" [level=1]
    text "Welcome back, Alex"
[2] link "Settings"
[3] link "Profile"
    navigation "Main":
[4]   link "Home"
[5]   link "Projects"
    main:
      heading "Recent Projects" [level=2]
[6]   textbox "Search" [focused] value="query"
[7]   button "Search"
```

format: `[ref] role "name" [attr=value]` with 2-space indentation for hierarchy. refs only on interactive roles.

### interactive roles (get refs)

button, link, textbox, combobox, checkbox, radio, slider, switch, menuitem, tab, treeitem, option, searchbox, spinbutton

### properties to include

- **always**: role, name (accessible name)
- **when present**: value (inputs), level (headings), checked/pressed/expanded/selected (stateful widgets), disabled, focused

### CDP approach

use `chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams` with depth limit (8-10). returns flat `Vec<AxNode>` — reconstruct tree via `parent_id`/`child_ids`. each node has `backend_dom_node_id` which maps back to the DOM for click/type targeting.

### ref → DOM mapping

maintain a `HashMap<u32, BackendNodeId>` per snapshot. when `click` or `type` receives a `ref` instead of `selector`, resolve the ref to a `BackendNodeId`, then use `DOM.resolveNode` to get a `RemoteObjectId` for interaction.

### size limits

- depth limit: 10 (via CDP `depth` param)
- max nodes: 500 after filtering — truncate with "... (N more nodes)"
- skip nodes with `ignored: true`
- collapse nodes with role `generic`/`none`/`group` that have no name (promote children)

### implementation

1. add `snapshot` action to `BrowserInput` and `browser_definition()` schema
2. implement `action_snapshot()`:
   - call `page.execute(GetFullAxTreeParams::builder().depth(10).build())`
   - filter and format the `Vec<AxNode>` into indented text
   - assign refs to interactive elements, store mapping in a `RefMap`
3. store the ref map in `BrowserState` (behind the existing mutex)
4. extend `action_click` and `action_type` to accept `ref` parameter:
   - if `ref` is provided, resolve via ref map → `BackendNodeId` → `DOM.resolveNode` → click/type
   - if `selector` is provided, use existing CSS selector path
5. add `ref` to the tool JSON schema as optional parameter for click/type

### test plan

- unit: format_ax_tree produces correct indented output with refs
- unit: interactive roles get refs, structural roles don't
- unit: collapsed structural nodes promote children
- unit: ref map stores correct backend node IDs
- unit: click/type with ref parameter validates ref exists
- integration: snapshot on a real page returns non-empty tree (requires chrome)
