use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Widget, WidgetRef};

use crate::app_event_sender::AppEventSender;
use crate::history_cell;
use crate::user_approval_widget::ApprovalRequest;
use crate::user_approval_widget::UserApprovalWidget;
use codex_common::approval_presets::ApprovalPreset;
use codex_common::approval_presets::builtin_approval_presets;
use codex_core::protocol::Op;

use super::BottomPane;
use super::BottomPaneView;
use super::CancellationEvent;

/// Modal overlay asking the user to approve/deny a sequence of requests.
pub(crate) struct ApprovalModalView {
    current: UserApprovalWidget,
    queue: Vec<ApprovalRequest>,
    app_event_tx: AppEventSender,
    mode: Mode,
    midturn_approval_mode_enabled: bool,
}

#[derive(Debug)]
enum Mode {
    /// Regular approval prompt flow.
    Prompt,
    /// Inline approval preset selector displayed over the modal.
    SelectingPresets {
        presets: Vec<ApprovalPreset>,
        selected: usize,
    },
}

impl ApprovalModalView {
    pub fn new(
        request: ApprovalRequest,
        app_event_tx: AppEventSender,
        midturn_approval_mode_enabled: bool,
    ) -> Self {
        Self {
            current: UserApprovalWidget::new(request, app_event_tx.clone()),
            queue: Vec::new(),
            app_event_tx,
            mode: Mode::Prompt,
            midturn_approval_mode_enabled,
        }
    }

    pub fn enqueue_request(&mut self, req: ApprovalRequest) {
        self.queue.push(req);
    }

    /// Advance to next request if the current one is finished.
    fn maybe_advance(&mut self) {
        if self.current.is_complete()
            && let Some(req) = self.queue.pop()
        {
            self.current = UserApprovalWidget::new(req, self.app_event_tx.clone());
        }
    }
}

impl BottomPaneView for ApprovalModalView {
    fn handle_key_event(&mut self, _pane: &mut BottomPane, key_event: KeyEvent) {
        match &mut self.mode {
            Mode::Prompt => {
                // Intercept 'c' to open the approval presets selector.
                if self.midturn_approval_mode_enabled
                    && matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('C'))
                {
                    let presets = builtin_approval_presets();
                    self.mode = Mode::SelectingPresets { presets, selected: 0 };
                    return;
                }
                self.current.handle_key_event(key_event);
                self.maybe_advance();
            }
            Mode::SelectingPresets { presets, selected } => {
                match key_event.code {
                    KeyCode::Esc => {
                        // Dismiss selector, return to approval prompt.
                        self.mode = Mode::Prompt;
                    }
                    KeyCode::Up | KeyCode::Left => {
                        if *selected == 0 { *selected = presets.len().saturating_sub(1); } else { *selected -= 1; }
                    }
                    KeyCode::Down | KeyCode::Right => {
                        *selected = (*selected + 1) % presets.len().max(1);
                    }
                    KeyCode::Enter => {
                        if let Some(preset) = presets.get(*selected) {
                            // Apply selection: update session approval/sandbox immediately for the remainder of the session/turn.
                            self.app_event_tx.send(crate::app_event::AppEvent::CodexOp(Op::OverrideTurnContext {
                                cwd: None,
                                approval_policy: Some(preset.approval),
                                sandbox_policy: Some(preset.sandbox.clone()),
                                model: None,
                                effort: None,
                                summary: None,
                            }));
                            // Update UI copy of config immediately.
                            self.app_event_tx
                                .send(crate::app_event::AppEvent::UpdateAskForApprovalPolicy(preset.approval));
                            self.app_event_tx
                                .send(crate::app_event::AppEvent::UpdateSandboxPolicy(preset.sandbox.clone()));
                            // Insert an informational history line. Note: takes effect after current prompt if one is in progress.
                            let msg = format!("Approval mode changed to '{}'; will apply after current prompt if one is in progress.", preset.label);
                            self.app_event_tx.send(crate::app_event::AppEvent::InsertHistoryCell(Box::new(
                                history_cell::new_info_event(msg, Some("Use /status to view current settings".to_string())),
                            )));
                        }
                        // Return to approval prompt.
                        self.mode = Mode::Prompt;
                    }
                    _ => {}
                }
            }
        }
    }

    fn on_ctrl_c(&mut self, _pane: &mut BottomPane) -> CancellationEvent {
        self.current.on_ctrl_c();
        self.queue.clear();
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.current.is_complete() && self.queue.is_empty()
    }

    fn desired_height(&self, width: u16) -> u16 {
        match &self.mode {
            Mode::Prompt => self.current.desired_height(width),
            Mode::SelectingPresets { presets, .. } => {
                // Header + items + footer (bounded to modal area width)
                let items = presets.len() as u16;
                items.saturating_add(4).min(width)
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        match &self.mode {
            Mode::Prompt => (&self.current).render_ref(area, buf),
            Mode::SelectingPresets { presets, selected } => {
                // Draw a simple inline selector box inside the existing modal area.
                let [title, list, footer] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(2),
                ])
                .areas(area.inner(ratatui::layout::Margin::new(2, 1)));

                Line::from("Select Approval Mode").bold().render(title, buf);
                // Render options one per line, highlighting selection.
                let mut y = list.y;
                for (i, p) in presets.iter().enumerate() {
                    let is_sel = i == *selected;
                    let marker = if is_sel { ">" } else { " " };
                    let text = format!("{} {} — {}", marker, p.label, p.description);
                    let mut line = Line::from(text);
                    if is_sel {
                        line = line.bold();
                    } else {
                        line = line.dim();
                    }
                    let row = Rect::new(list.x, y, list.width, 1);
                    line.render(row, buf);
                    y = y.saturating_add(1);
                    if y >= list.y.saturating_add(list.height) { break; }
                }
                let hint1 = "Enter: apply   Esc: back   C: cancel";
                let hint2 = "Note: changes apply after current prompt, if one is in progress";
                Line::from(hint1).italic().dim().render(footer, buf);
                let footer2 = Rect::new(footer.x, footer.y.saturating_add(1), footer.width, 1);
                Line::from(hint2).italic().dim().render(footer2, buf);
            }
        }
    }

    fn try_consume_approval_request(&mut self, req: ApprovalRequest) -> Option<ApprovalRequest> {
        self.enqueue_request(req);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use tokio::sync::mpsc::unbounded_channel;

    fn make_exec_request() -> ApprovalRequest {
        ApprovalRequest::Exec {
            id: "test".to_string(),
            command: vec!["echo".to_string(), "hi".to_string()],
            reason: None,
        }
    }

    #[test]
    fn ctrl_c_aborts_and_clears_queue() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx);
        let first = make_exec_request();
        let mut view = ApprovalModalView::new(first, tx, false);
        view.enqueue_request(make_exec_request());

        let (tx2, _rx2) = unbounded_channel::<AppEvent>();
        let mut pane = BottomPane::new(super::super::BottomPaneParams {
            app_event_tx: AppEventSender::new(tx2),
            frame_requester: crate::tui::FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            midturn_approval_mode_enabled: false,
        });
        assert_eq!(CancellationEvent::Handled, view.on_ctrl_c(&mut pane));
        assert!(view.queue.is_empty());
        assert!(view.current.is_complete());
        assert!(view.is_complete());
    }
}
