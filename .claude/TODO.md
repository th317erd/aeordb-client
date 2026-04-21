# TODO — File Browser Multi-Selection

## Phase 1: Selection State + Visual — DONE
- [x] `selectedEntries` (Set) and `lastSelectedIndex` on tab objects
- [x] Plain click = single select + preview (dirs still navigate)
- [x] Ctrl/Meta+Click = toggle selection
- [x] Shift+Click = range select from anchor
- [x] `.selected` CSS class with accent highlight
- [x] Selection bar shows count + Clear + Delete Selected buttons
- [x] Ctrl+A selects all, Escape clears
- [x] Selection persists across pagination (name-based Set)
- [x] Selection cleared on directory navigation

## Phase 2: Bulk Actions — DONE
- [x] Bulk delete via selection bar or context menu
- [x] Bulk move: drag multiple selected onto a folder
- [x] Multi-entry drag data (`application/x-aeordb-entries`)
- [x] `file-drag-start` event includes all selected paths/entries

## Phase 3: Integration — DONE
- [x] Plain click still previews single files
- [x] Ctrl/Shift clicks select without previewing
- [x] Context menu: bulk menu when right-clicking selected entry in multi-select
- [x] Context menu: selects entry first when right-clicking unselected
