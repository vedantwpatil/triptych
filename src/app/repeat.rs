//! Repeating tasks: finishing one adds its next occurrence (rules in `crate::recurrence`).

use chrono::Local;

use super::{
    App,
    model::{TASK_COLUMNS, Task},
};
use crate::recurrence::Recurrence;

impl App {
    /// Call right after task `id` was marked done. When it has a repeat rule, inserts the next
    /// occurrence (same text, priority, tags, duration and links; dates moved by the rule) at the
    /// bottom of the list. Does nothing if an open copy with those dates exists already, so
    /// done, reopen, done never doubles it.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub(super) async fn add_next_occurrence(&mut self, id: i64) -> Result<(), sqlx::Error> {
        let task =
            sqlx::query_as::<_, Task>(&format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.db_pool)
                .await?;
        let Some(task) = task else {
            return Ok(());
        };
        let Some(rule) = task.recurrence.as_deref().and_then(Recurrence::from_key) else {
            return Ok(());
        };
        let (scheduled, deadline) =
            rule.next_dates(task.scheduled_at, task.deadline, crate::clock::now());

        let exists: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM tasks WHERE completed = 0 AND recurrence = ? AND description = ? \
             AND scheduled_at IS ? AND deadline IS ?",
        )
        .bind(rule.key())
        .bind(&task.description)
        .bind(scheduled)
        .bind(deadline)
        .fetch_optional(&self.db_pool)
        .await?;
        if exists.is_some() {
            return Ok(());
        }

        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, natural_language_input, \
             tags, scheduled_at, deadline, duration_minutes, task_category, links, recurrence) \
             SELECT description, 0, (SELECT COALESCE(MAX(item_order), -1) + 1 FROM tasks), priority, \
             natural_language_input, tags, ?, ?, duration_minutes, task_category, links, recurrence \
             FROM tasks WHERE id = ?",
        )
        .bind(scheduled)
        .bind(deadline)
        .bind(id)
        .execute(&self.db_pool)
        .await?;

        let next = scheduled.or(deadline).map_or_else(String::new, |d| {
            format!(": {}", d.with_timezone(&Local).format("%a %b %-d"))
        });
        self.status_message = Some((
            format!("Repeats {}, next{next}", rule.key()),
            std::time::Instant::now(),
        ));
        Ok(())
    }
}
