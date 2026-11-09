//! The Notifications tool window: the event log, with unread markers,
//! "Mark all as read" and "Clear all".

use ferro_services::{NotificationEntry, NotificationLevel, Services};
use ferro_ui::{FerroIcon, components, tr};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window,
    component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
    px,
};

use super::common;

pub struct NotificationsView {
    services: Services,
    entries: Vec<NotificationEntry>,
    focus_handle: FocusHandle,
}

pub fn build(services: Services, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| NotificationsView::new(services, cx)).into()
}

/// "Now" for relative times. The mock's entries are relative to its fixed
/// clock, so the mock clock is used while running on mocks.
fn now() -> i64 {
    ferro_services::mock::MOCK_NOW
}

impl NotificationsView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            entries: Vec::new(),
            services,
            focus_handle: cx.focus_handle(),
        };
        this.reload();
        this
    }

    pub fn reload(&mut self) {
        let mut entries = self.services.status.notifications();
        entries.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        self.entries = entries;
    }

    pub fn entries(&self) -> &[NotificationEntry] {
        &self.entries
    }

    pub fn unread(&self) -> usize {
        self.entries.iter().filter(|e| !e.read).count()
    }

    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        self.services.status.mark_notifications_read();
        self.reload();
        cx.notify();
    }

    pub fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.services.status.clear_notifications();
        self.reload();
        cx.notify();
    }
}

impl Focusable for NotificationsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NotificationsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, primary) = (theme.muted_foreground, theme.border, theme.primary);
        let toolbar = common::toolbar("notifications-toolbar", cx)
            .child(div().flex_1())
            .child(
                common::tool_button(
                    "notifications-read",
                    FerroIcon::Checklist,
                    tr("panels-notifications-mark-read"),
                )
                .label(tr("panels-notifications-mark-read"))
                .disabled(self.unread() == 0)
                .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
            )
            .child(
                common::tool_button(
                    "notifications-clear",
                    FerroIcon::Delete,
                    tr("panels-notifications-clear"),
                )
                .label(tr("panels-notifications-clear"))
                .disabled(self.entries.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.clear_all(cx))),
            );
        let body = if self.entries.is_empty() {
            components::empty_state(
                FerroIcon::Notifications,
                tr("panels-notifications-empty"),
                None,
                cx,
            )
            .into_any_element()
        } else {
            let now = now();
            v_flex()
                .id("notifications-list")
                .size_full()
                .overflow_y_scroll()
                .children(self.entries.iter().map(|entry| {
                    let (icon, color) = match entry.level {
                        NotificationLevel::Info => (FerroIcon::Info, cx.theme().info),
                        NotificationLevel::Success => (FerroIcon::Ok, cx.theme().success),
                        NotificationLevel::Warning => (FerroIcon::Warning, cx.theme().warning),
                        NotificationLevel::Error => (FerroIcon::Error, cx.theme().danger),
                    };
                    h_flex()
                        .id(("notification", entry.id as usize))
                        .items_start()
                        .gap_2()
                        .px_2()
                        .py_2()
                        .border_b_1()
                        .border_color(border)
                        .aria_label(SharedString::from(entry.title.clone()))
                        .child(Icon::from(icon).small().text_color(color))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .text_sm()
                                        .child(div().flex_1().truncate().child(entry.title.clone()))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(common::relative_time(entry.timestamp, now)),
                                        ),
                                )
                                .when(!entry.body.is_empty(), |el| {
                                    el.child(
                                        div().text_xs().text_color(muted).child(entry.body.clone()),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .size(px(7.))
                                .mt_1()
                                .rounded_full()
                                .when(!entry.read, |el| el.bg(primary)),
                        )
                }))
                .into_any_element()
        };
        v_flex()
            .id("notifications")
            .track_focus(&self.focus_handle)
            .aria_label(tr("tool-notifications"))
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }
}
