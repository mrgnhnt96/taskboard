//! The pages, panels and dialogs of the main window. See `app.rs` for how they fit together.
pub mod backlog;
pub mod board;
pub mod days;
pub mod goal;
pub mod goals;
pub mod goals_page;
pub mod home;
pub mod hours;
pub mod idea;
pub mod ideas;
pub mod issue_panel;
pub mod kit;
pub mod md;
pub mod paged;
pub mod modals;
pub mod projects;
pub mod prwatch;
pub mod releases;
pub mod review;
pub mod search;
pub mod select;
pub mod sidebar;
pub mod status;
pub mod task_panel;
pub mod text_input;

/// Both the same answer, or both none: what a page built from one still holds for the other.
pub fn same<T>(a: &Option<std::rc::Rc<T>>, b: &Option<std::rc::Rc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::rc::Rc::ptr_eq(a, b),
        (a, b) => a.is_none() && b.is_none(),
    }
}
