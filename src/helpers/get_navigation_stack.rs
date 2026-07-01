use gtk4::{ApplicationWindow, Box};
use vte4::{CastNone, GtkWindowExt, WidgetExt};

pub fn get_navigation_stack(widget: &impl WidgetExt) -> Option<(Box, ApplicationWindow)> {
    let Some(window) = widget.root().and_downcast::<ApplicationWindow>() else {
        return None;
    };
    let Some(main_container) = window.child().and_downcast::<Box>() else {
        return None;
    };
    let Some(content_box) = main_container.first_child().and_downcast::<Box>() else {
        return None;
    };

    return Some((content_box, window));
}
