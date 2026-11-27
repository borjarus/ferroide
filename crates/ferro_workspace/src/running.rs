//! Running from the workspace (Stage 8): ▶ icons in the editor, build tool
//! tasks, rerunning failed tests and Run Context Configuration.
//!
//! Everything that starts a process checks trust first (an untrusted
//! project runs nothing), starts a run through `RunService`, selects the
//! new configuration in the title bar's run selector and shows the Run
//! tool window.

use ferro_services::{NewTerminal, RunState, Runnable};
use ferro_ui::tr;
use gpui_kit::{Context, Window};

use crate::{
    panels::{run::RunView, terminal::TerminalView},
    requests::{RunBuildTask, RunRunnable},
    tool_window::ToolWindowKind,
    workspace::Workspace,
};

impl Workspace {
    /// Refuse to run anything in an untrusted project, with a notice.
    fn ensure_trusted(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if crate::projects::is_trusted(&self.services, cx) {
            return true;
        }
        crate::notify_warning(window, cx, tr("project-untrusted-run").to_string());
        false
    }

    /// After a run started: select it and show where its output goes.
    pub(crate) fn run_started(
        &mut self,
        id: &str,
        debug: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_run_configs(cx);
        if let Some(ix) = self.run_configs.iter().position(|c| c.id == id) {
            self.selected_run = ix;
            self.run_state = self.services.run.state(id);
        }
        let kind = if debug {
            ToolWindowKind::Debug
        } else {
            ToolWindowKind::Run
        };
        if !self.is_tool_window_visible(kind, cx) {
            self.show_tool_window(kind, window, cx);
        }
        cx.notify();
    }

    /// Run (or debug) a ▶ runnable of an editor.
    pub fn run_runnable(
        &mut self,
        request: &RunRunnable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_trusted(window, cx) {
            return;
        }
        match self
            .services
            .run
            .run_runnable(&request.path, &request.runnable, request.debug)
        {
            Ok(id) => self.run_started(&id, request.debug, window, cx),
            Err(err) => crate::notify_warning(window, cx, format!("{err:#}")),
        }
    }

    /// Run a build tool task in the Run tool window or a new terminal.
    pub fn run_build_task(
        &mut self,
        request: &RunBuildTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_trusted(window, cx) {
            return;
        }
        if request.in_terminal {
            let options = NewTerminal {
                cwd: Some(request.task.working_directory.clone()),
                title: Some(request.task.name.clone()),
                command: Some(request.task.command.clone()),
                ..Default::default()
            };
            self.show_tool_window(ToolWindowKind::Terminal, window, cx);
            let opened =
                match self.tool_window_body::<TerminalView>(ToolWindowKind::Terminal, window, cx) {
                    Some(view) => view.update(cx, |view, cx| view.open(options, cx)),
                    None => self.services.terminal.open(options).map(|_| ()),
                };
            if let Err(err) = opened {
                crate::notify_warning(window, cx, format!("{err:#}"));
            }
            return;
        }
        match self.services.run.run_task(&request.task) {
            Ok(id) => self.run_started(&id, false, window, cx),
            Err(err) => crate::notify_warning(window, cx, format!("{err:#}")),
        }
    }

    /// Rerun the failed tests of the selected configuration's last run.
    pub fn rerun_failed_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ensure_trusted(window, cx) {
            return;
        }
        // The run shown in the Run tool window first, then the selected one.
        let shown = self
            .tool_window_body::<RunView>(ToolWindowKind::Run, window, cx)
            .and_then(|view| view.read(cx).active_run().and_then(|r| r.id.clone()));
        let Some(id) = shown.or_else(|| {
            self.run_configs
                .get(self.selected_run)
                .map(|c| c.id.clone())
        }) else {
            return;
        };
        if matches!(self.services.run.state(&id), RunState::Running { .. }) {
            return;
        }
        match self.services.run.rerun_failed(&id) {
            Ok(id) => self.run_started(&id, false, window, cx),
            Err(err) => crate::notify_warning(window, cx, format!("{err:#}")),
        }
    }

    /// Run the ▶ runnable at the caret (the nearest one above it): the test
    /// or `main` the caret is in. Without one, run the selected
    /// configuration.
    pub fn run_at_caret(&mut self, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.active_editor_entity().and_then(|editor| {
            let editor = editor.read(cx);
            let line = editor.caret(cx)?.line;
            let runnable = runnable_at(&editor.analysis().runnables, line)?;
            Some((editor.path().to_path_buf(), runnable))
        });
        match found {
            Some((path, runnable)) => self.run_runnable(
                &RunRunnable {
                    path,
                    runnable,
                    debug,
                },
                window,
                cx,
            ),
            None => self.start_run(debug, window, cx),
        }
    }
}

/// The runnable whose line is the nearest at or above `line`.
pub fn runnable_at(runnables: &[Runnable], line: u32) -> Option<Runnable> {
    runnables
        .iter()
        .filter(|r| r.line <= line)
        .max_by_key(|r| r.line)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferro_services::RunnableKind;

    #[test]
    fn the_nearest_runnable_above_the_caret_wins() {
        let r = |line, label: &str| Runnable {
            line,
            kind: RunnableKind::Test,
            label: label.into(),
        };
        let all = [r(2, "a"), r(10, "b"), r(20, "c")];
        assert_eq!(runnable_at(&all, 12).unwrap().label, "b");
        assert_eq!(runnable_at(&all, 10).unwrap().label, "b");
        assert!(runnable_at(&all, 1).is_none());
    }
}
