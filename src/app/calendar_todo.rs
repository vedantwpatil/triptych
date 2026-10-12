//! The calendar's side to-do panel: open tasks beside the grid, schedulable at the cursor cell.

use super::{
    App,
    model::{CalendarInputMode, Task},
};

impl App {
    /// Open tasks in todo-list order: what the panel lists.
    #[must_use]
    pub fn calendar_todo_tasks(&self) -> Vec<&Task> {
        self.tasks.iter().filter(|t| !t.completed).collect()
    }

    /// `T` on the grid: shows the panel and routes keys into it.
    pub fn focus_calendar_todo(&mut self) {
        self.calendar_todo_open = true;
        self.clamp_calendar_todo();
        self.calendar_input_mode = CalendarInputMode::TodoPanel;
    }

    /// `T` in the panel: hides it and returns keys to the grid.
    pub const fn close_calendar_todo(&mut self) {
        self.calendar_todo_open = false;
        self.calendar_input_mode = CalendarInputMode::Navigate;
    }

    /// Moves the panel cursor one row, stopping at either end.
    pub fn calendar_todo_step(&mut self, forward: bool) {
        if forward {
            self.calendar_todo_selected += 1;
        } else {
            self.calendar_todo_selected = self.calendar_todo_selected.saturating_sub(1);
        }
        self.clamp_calendar_todo();
    }

    /// Schedules the highlighted task at the grid cursor's cell (a scheduled task moves there)
    /// and hands keys back to the grid so the cursor can pick the next cell.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn schedule_calendar_todo_task(&mut self) -> Result<(), sqlx::Error> {
        let Some(id) = self.calendar_todo_task_id() else {
            return Ok(());
        };
        self.schedule_task_at_selected_cell(id).await?;
        // `load_tasks` re-sorts by urgency, which a new `scheduled_at` can raise: follow the task.
        if let Some(pos) = self.calendar_todo_tasks().iter().position(|t| t.id == id) {
            self.calendar_todo_selected = pos;
        }
        self.calendar_input_mode = CalendarInputMode::Navigate;
        self.status_message = Some(("Task scheduled".to_string(), std::time::Instant::now()));
        Ok(())
    }

    /// Marks the highlighted task done (a repeating task adds its next copy) and keeps the cursor
    /// on the row that takes its place.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn complete_calendar_todo_task(&mut self) -> Result<(), sqlx::Error> {
        let Some(id) = self.calendar_todo_task_id() else {
            return Ok(());
        };
        self.complete_task_by_id(id).await?;
        self.clamp_calendar_todo();
        Ok(())
    }

    fn calendar_todo_task_id(&self) -> Option<i64> {
        self.calendar_todo_tasks()
            .get(self.calendar_todo_selected)
            .map(|t| t.id)
    }

    fn clamp_calendar_todo(&mut self) {
        let last = self.calendar_todo_tasks().len().saturating_sub(1);
        self.calendar_todo_selected = self.calendar_todo_selected.min(last);
    }
}
