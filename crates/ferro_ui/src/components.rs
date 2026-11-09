//! Small building blocks shared by the workspace, the welcome screen and the
//! storybook. Each is a thin, consistently styled wrapper over a GPUI Kit
//! component.

use gpui_kit::{
    App, ElementId, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    component::{
        ActiveTheme as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::FerroIcon;

/// A panel toggle in the status bar (Zed): the panel's icon, highlighted
/// while the panel is on screen; a click shows or hides it.
pub fn panel_toggle(
    id: impl Into<ElementId>,
    icon: FerroIcon,
    tooltip: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .icon(Icon::from(icon))
        .ghost()
        .xsmall()
        .selected(active)
        .tooltip(tooltip)
        .on_click(on_click)
}

/// A title-bar picker: optional icon, a label and a dropdown caret.
pub fn toolbar_widget(
    id: impl Into<ElementId>,
    icon: Option<FerroIcon>,
    label: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .when_some(icon, |button, icon| button.icon(Icon::from(icon)))
        .label(label)
        .dropdown_caret(true)
}

/// A plain icon button for toolbars.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: FerroIcon,
    tooltip: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .icon(Icon::from(icon))
        .ghost()
        .small()
        .tooltip(tooltip)
}

/// A clickable status-bar segment.
pub fn status_item(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().xsmall().label(label)
}

/// A muted, centered message for empty and error states.
pub fn empty_state(
    icon: FerroIcon,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_4()
        .text_color(theme.muted_foreground)
        .child(Icon::from(icon).size(px(28.)))
        .child(div().text_sm().child(title.into()))
        .when_some(detail, |el, detail| el.child(div().text_xs().child(detail)))
}

/// A thin horizontal progress bar for background tasks.
pub fn progress_bar(fraction: f32, width: gpui_kit::Pixels, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w(width)
        .h(px(4.))
        .rounded_full()
        .bg(theme.border)
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(theme.primary)
                .w(width * fraction.clamp(0.0, 1.0)),
        )
}
