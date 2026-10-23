//! The `?` key reference: one static page per view, drawn over whatever is on screen.

use super::centered_rect;
use crate::app::{App, ViewMode};
use ratatui::{
    Frame,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

const TODO: &str = "\
Move      j/k  gg/G  Ctrl-d/u  counts (5j)
Tasks     a add  r reword  Enter done/undone  x/d/D delete  u undo delete
Select    v range (j/k extend, d delete, Esc cancel)
Plan      s auto-schedule
Find      / search  n/N next/previous
Links     o open first  O list (Enter or 1-9 opens)
Apply     A fill a job application (y submit, r recheck, n cancel)
Views     c calendar  m email  Tab/Shift-Tab cycle  q quit";

const CALENDAR: &str = "\
Move      h/j/k/l  H/L week  0/$ first/last day  gg/G
Blocks    n new block  d delete block
Tasks     s pick task  a add here  T to-do panel  m pick up/drop
          u unschedule  e deadline  [ ] cycle stacked tasks
Inspect   Enter/v cell details
Views     t/Esc to-do  Tab/Shift-Tab cycle  q quit";

const EMAIL: &str = "\
Move      j/k  gg/G  / search  n/N  o sort  s sync
Read      v open  r read  u unread  f star  t category
Message   R reply  A reply-all  F forward  s save attachments
Act       c compose  Enter to task  M meeting to task  a archive  d delete (twice)
Filters   A account  F folder  I focus  H attach  U unread  S starred  @ domain
Mailbox   B folders  D drafts  R rules  z snooze  Z snoozed  x unsnooze
Compose   Tab field  Ctrl-S send  Ctrl-D draft  Ctrl-N/P contacts  Esc cancel
Views     m/Esc to-do  Tab/Shift-Tab cycle";

pub(super) fn render_help_popup(f: &mut Frame, app: &App) {
    let (name, text) = match app.view_mode {
        ViewMode::TodoList => ("To-Do", TODO),
        ViewMode::Calendar => ("Calendar", CALENDAR),
        ViewMode::Email => ("Email", EMAIL),
    };
    let area = centered_rect(90, 70, f.area());
    f.render_widget(Clear, area);
    let help = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Keys: {name}"))
                .title_bottom("any key closes")
                .style(Style::default().bg(Color::Black)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(help, area);
}
