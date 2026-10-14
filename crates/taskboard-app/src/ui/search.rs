//! Search (⌘K and the sidebar's search box). For now it opens the Tasks page; the palette comes
//! with T257.
use crate::app::{MainWindow, Page};
use gpui_kit::*;

pub fn open(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) {
    m.go(Page::Tasks, cx);
}
