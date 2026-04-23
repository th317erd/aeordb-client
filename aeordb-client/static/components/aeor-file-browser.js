'use strict';

// Migrate old localStorage format before the shared component loads.
// Old client used camelCase keys (relationshipId, viewMode, etc.)
// New shared base uses snake_case (relationship_id, view_mode, etc.)
(function migrateLocalStorage() {
  try {
    const raw = localStorage.getItem('aeordb-file-browser');
    if (!raw) return;

    const state = JSON.parse(raw);
    if (!state.tabs || !Array.isArray(state.tabs)) return;

    let migrated = false;

    state.tabs = state.tabs
      .map((tab) => {
        // Migrate camelCase → snake_case
        if (tab.relationshipId && !tab.relationship_id) {
          tab.relationship_id = tab.relationshipId;
          delete tab.relationshipId;
          migrated = true;
        }
        if (tab.relationshipName && !tab.relationship_name) {
          tab.relationship_name = tab.relationshipName;
          delete tab.relationshipName;
          migrated = true;
        }
        if (tab.viewMode && !tab.view_mode) {
          tab.view_mode = tab.viewMode;
          delete tab.viewMode;
          migrated = true;
        }
        if (tab.pageSize && !tab.page_size) {
          tab.page_size = tab.pageSize;
          delete tab.pageSize;
          migrated = true;
        }
        if (tab.previewHeight && !tab.preview_height) {
          tab.preview_height = tab.previewHeight;
          delete tab.previewHeight;
          migrated = true;
        }
        // Migrate activeTabId → active_tab_id at top level
        return tab;
      })
      .filter((tab) => tab.relationship_id); // drop corrupt tabs

    if (state.activeTabId && !state.active_tab_id) {
      state.active_tab_id = state.activeTabId;
      delete state.activeTabId;
      migrated = true;
    }
    if (state.tabCounter && !state.tab_counter) {
      state.tab_counter = state.tabCounter;
      delete state.tabCounter;
      migrated = true;
    }

    if (migrated) {
      localStorage.setItem('aeordb-file-browser', JSON.stringify(state));
    }
  } catch (e) {
    // If migration fails, clear it entirely
    localStorage.removeItem('aeordb-file-browser');
  }
})();

// Re-export the shared client file browser.
export { AeorFileBrowser } from '../shared/components/aeor-file-browser.js';
