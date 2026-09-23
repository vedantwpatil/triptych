//! `/` search over the todo and email lists, with `n`/`N` to repeat it.

use super::{App, InputMode, ViewMode, motion::find_match};

impl App {
    /// Starts typing a search; `commit_search` or `cancel_search` ends it.
    pub fn start_search(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::Search;
    }

    pub fn cancel_search(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Ends typing and jumps to the first match after the cursor. An empty query repeats the last one.
    pub fn commit_search(&mut self) {
        let typed = self.input_buffer.trim().to_string();
        self.cancel_search();
        if !typed.is_empty() {
            self.search_query = typed;
        }
        self.search_step(true);
    }

    /// Jumps to the next (`forward`) or previous match of the last query in the current list view.
    pub fn search_step(&mut self, forward: bool) {
        if self.search_query.is_empty() {
            self.notify("No previous search");
            return;
        }
        let needle = self.search_query.to_lowercase();
        let (len, cur) = match self.view_mode {
            ViewMode::TodoList => (self.tasks.len(), self.selected),
            ViewMode::Email => (self.emails.len(), self.selected_email),
            ViewMode::Calendar => return,
        };
        if len == 0 {
            self.notify(&format!("Pattern not found: {}", self.search_query));
            return;
        }
        let todo = self.view_mode == ViewMode::TodoList;
        let hit = find_match(len, cur, forward, |i| {
            if todo {
                self.tasks[i].description.to_lowercase().contains(&needle)
            } else {
                let e = &self.emails[i];
                let from = e.from_name.as_deref().unwrap_or(&e.from_addr);
                format!("{} {from}", e.subject).to_lowercase().contains(&needle)
            }
        });
        let Some((idx, wrapped)) = hit else {
            self.notify(&format!("Pattern not found: {}", self.search_query));
            return;
        };
        if todo {
            self.selected = idx;
        } else {
            self.selected_email = idx;
        }
        if wrapped {
            self.notify(if forward {
                "Search hit BOTTOM, continuing at TOP"
            } else {
                "Search hit TOP, continuing at BOTTOM"
            });
        }
    }
}
