//! The menu bar: every page and command with its key beside it. An item is greyed while it has
//! nothing to act on: its handler is only put on the window while it can run, and macOS asks
//! the window (`is_action_available`) before it shows a menu.
use crate::app::{self, MainWindow, Page, Panel};
use crate::fmt::{self, obj, opt_s, s};
use crate::ui;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

actions!(
    taskboard,
    [
        FocusInMidna,
        OpenPr,
        StartTask,
        StopTask,
        DoneTask,
        IdeaDig,
        IdeaPlan,
        IdeaTalk,
        IdeaShelve,
        IdeaDrop,
        IdeaReopen,
        Minimize,
        Zoom,
        BringAllToFront,
        ShowWindow,
        About,
        CheckForUpdates,
        InstallHooks,
        RestartDaemon,
        OpenLog,
        EditCut,
        EditPaste,
        EditSelectAll,
    ]
);

/// Keys for the menu's own commands. Cut, Paste and Select All are the text field's keys; their
/// bindings sit in a context nothing has, only so the menu shows the key.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-enter", OpenPr, Some("MainWindow")),
        KeyBinding::new("cmd-d", IdeaDig, Some("MainWindow")),
        KeyBinding::new("cmd-shift-p", IdeaPlan, Some("MainWindow")),
        KeyBinding::new("cmd-t", IdeaTalk, Some("MainWindow")),
        KeyBinding::new("cmd-m", Minimize, None),
        // ⌘↩ in a field sends what's typed (an answer, New idea's details).
        KeyBinding::new("cmd-enter", NoAction, Some(ui::text_input::CTX)),
        KeyBinding::new("cmd-x", EditCut, Some(MENU_ONLY)),
        KeyBinding::new("cmd-v", EditPaste, Some(MENU_ONLY)),
        KeyBinding::new("cmd-a", EditSelectAll, Some(MENU_ONLY)),
    ]);
}

const MENU_ONLY: &str = "MenuKeyOnly";

/// The app-wide commands: the window, About, updates and the log.
pub fn on_app_actions(fake: bool, cx: &mut App) {
    cx.on_action(|_: &Minimize, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.zoom_window());
        }
    });
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    cx.on_action(|_: &About, _| about());
    if crate::install::installed_bundle().is_some() {
        cx.on_action(|_: &CheckForUpdates, _| crate::updater::check_now());
    }
    if !fake {
        cx.on_action(|_: &OpenLog, _| {
            let data = taskboardd::config::Config::load().map(|c| c.data).unwrap_or_else(|_| taskboardd::config::default_data_dir());
            let _ = std::process::Command::new("open").arg("-a").arg("Console").arg(data.join("app.log")).spawn();
        });
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn about() {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    unsafe {
        let app: Option<Retained<AnyObject>> = msg_send![class!(NSApplication), sharedApplication];
        let Some(app) = app else { return };
        let _: () = msg_send![&*app, orderFrontStandardAboutPanel: std::ptr::null::<AnyObject>()];
    }
}
#[cfg(not(all(target_os = "macos", not(test))))]
fn about() {}

/// The pages in the sidebar's order, with the action that opens each.
fn page_items(here: &Page) -> Vec<MenuItem> {
    let on = |p: &[Page]| p.iter().any(|x| std::mem::discriminant(x) == std::mem::discriminant(here));
    vec![
        MenuItem::action("Needs you", app::GoBoard).checked(on(&[Page::Board])),
        MenuItem::action("Projects", app::GoProjects).checked(on(&[Page::Projects])),
        MenuItem::action("Goals", app::GoGoals).checked(on(&[Page::Goals, Page::Goal(String::new())])),
        MenuItem::action("Backlog", app::GoBacklog).checked(on(&[Page::Backlog])),
        MenuItem::action("Ideas", app::GoIdeas).checked(on(&[Page::Ideas, Page::Idea(String::new())])),
        MenuItem::action("Days", app::GoDays).checked(on(&[Page::Days, Page::Review, Page::Recap])),
        MenuItem::action("Releases", app::GoReleases).checked(on(&[Page::Releases, Page::ReleasesOf(String::new())])),
    ]
}

/// The whole menu bar. `here` checks its page in View; `name` is the window's line in Window.
pub fn menus(fake: bool, here: &Page, name: &str) -> Vec<Menu> {
    let mut view = page_items(here);
    view.extend([
        MenuItem::separator(),
        MenuItem::action("Search…", app::Search),
        MenuItem::separator(),
        MenuItem::action("Back", app::NavBack),
        MenuItem::action("Forward", app::NavForward),
        MenuItem::separator(),
        MenuItem::action("Refresh", app::Refresh),
    ]);
    let mut help = vec![MenuItem::action("Open Log", OpenLog)];
    if !fake {
        help.push(MenuItem::action("Export for Debugging…", crate::ExportForDebugging));
    }
    vec![
        Menu::new("Task board").items([
            MenuItem::action("About Task board", About),
            MenuItem::separator(),
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::action("Settings…", crate::OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Install Hooks", InstallHooks),
            MenuItem::action("Restart Daemon", RestartDaemon),
            MenuItem::separator(),
            MenuItem::action("Hide Task board", crate::Hide),
            MenuItem::action("Hide Others", crate::HideOthers),
            MenuItem::action("Show All", crate::ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Task board", crate::Quit),
        ]),
        Menu::new("File").items([MenuItem::action("New Idea…", app::NewIdea), MenuItem::separator(), MenuItem::action("Close Window", crate::CloseWindow)]),
        Menu::new("Edit").items([
            MenuItem::os_action("Cut", EditCut, OsAction::Cut),
            MenuItem::os_action("Copy", app::CopySelection, OsAction::Copy),
            MenuItem::os_action("Paste", EditPaste, OsAction::Paste),
            MenuItem::os_action("Select All", EditSelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Copy Reference", app::CopyRef),
        ]),
        Menu::new("View").items(view),
        Menu::new("Task").items([
            MenuItem::action("Focus in Midna", FocusInMidna),
            MenuItem::action("Open Pull Request", OpenPr),
            MenuItem::separator(),
            MenuItem::action("Start", StartTask),
            MenuItem::action("Stop", StopTask),
            MenuItem::action("Done", DoneTask),
        ]),
        Menu::new("Idea").items([
            MenuItem::action("Dig", IdeaDig),
            MenuItem::action("Plan", IdeaPlan),
            MenuItem::action("Talk It Over", IdeaTalk),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Close Idea").items([MenuItem::action("Shelve for Later", IdeaShelve), MenuItem::action("Won’t Do", IdeaDrop)])),
            MenuItem::action("Reopen Idea", IdeaReopen),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Bring All to Front", BringAllToFront),
            MenuItem::separator(),
            MenuItem::action(format!("{name} — Task board"), ShowWindow).checked(true),
        ]),
        Menu::new("Help").items(help),
    ]
}

// ------------------------------------------------------------------ what's open

/// The open task, once the panel has it from the board.
pub fn open_task(m: &MainWindow) -> Option<&Value> {
    let Some(Panel::Task { r, .. }) = &m.panel else { return None };
    m.data.task.as_ref().filter(|t| fmt::same_task(&fmt::ref_of(t, "T"), r))
}

/// The idea whose page is open, once the board has sent it.
pub fn open_idea(m: &MainWindow) -> Option<&Value> {
    let Page::Idea(r) = &m.page else { return None };
    m.data.idea.as_ref().filter(|i| fmt::ref_of(i, "I") == *r)
}

/// What the Task menu can do for a task.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskCan {
    pub focus: bool,
    pub pr: Option<String>,
    pub start: bool,
    /// Stop closes its terminal; `force` when the terminal is busy.
    pub stop: Option<bool>,
    pub done: bool,
}

pub fn task_can(t: &Value) -> TaskCan {
    let status = s(t, "status");
    let session = obj(t, "session").filter(|x| s(x, "status") != "gone");
    let retrying = opt_s(t, "start_retry_at").is_some() && status == "queued" && t["starting"] != true;
    TaskCan {
        focus: session.is_some(),
        pr: obj(t, "pr").filter(|p| p.get("num").is_some_and(|n| !n.is_null())).and_then(|p| opt_s(p, "url")).map(str::to_string),
        start: matches!(status, "queued" | "planned") && obj(t, "goal").is_none() && t["starting"] != true && !retrying,
        stop: session.map(|x| s(x, "status") != "idle"),
        done: !matches!(status, "done" | "cancelled" | ""),
    }
}

/// What the Idea menu can do for an idea: Dig, Plan, Talk, Close and Reopen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IdeaCan {
    pub dig: bool,
    pub plan: bool,
    pub talk: bool,
    pub close: bool,
    pub reopen: bool,
}

pub fn idea_can(i: &Value) -> IdeaCan {
    let close = ui::idea::close_button(i).filter(|b| b.enabled);
    let reopen = ui::idea::close_menu_view(i).iter().any(|(a, _)| *a == "reopen");
    IdeaCan {
        dig: ui::idea::dig_button(i, fmt::now()).enabled,
        plan: matches!(ui::idea::plan_slot(i), ui::idea::PlanSlot::Button(b) if b.enabled),
        talk: ui::idea::talk_button(i).is_some_and(|b| b.enabled),
        close: close.is_some() && !reopen,
        reopen: close.is_some() && reopen,
    }
}

// ------------------------------------------------------------------ handlers

/// Puts each command's handler on the window while it can run, so its item and key are live
/// only then. Page keys, Search and New idea pause while a dialog is open.
pub fn on_actions(m: &MainWindow, d: Stateful<Div>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let keys = m.page_keys();
    let task = keys.then(|| open_task(m).map(|t| (fmt::ref_of(t, "T"), task_can(t)))).flatten();
    let idea = keys.then(|| open_idea(m).map(|i| (fmt::ref_of(i, "I"), idea_can(i)))).flatten();
    let tc = task.as_ref().map(|(_, c)| c.clone()).unwrap_or_default();
    let ic = idea.as_ref().map(|(_, c)| c.clone()).unwrap_or_default();
    let tr = task.map(|(r, _)| r).unwrap_or_default();
    let ir = idea.map(|(r, _)| r).unwrap_or_default();
    let hooks = !m.hooks_busy
        && matches!(m.hooks, crate::hooks::Hooks::NotInstalled | crate::hooks::Hooks::NeedsReinstall(_))
        && crate::hooks::target().is_some_and(|t| t.can_install);
    let restart = !m.daemon_busy && crate::install::installed_bundle().is_some();
    let page = |a: &dyn Action, m: &mut MainWindow, w: &mut Window, cx: &mut Context<MainWindow>| m.on_page_key(a, w, cx);

    d.on_action(cx.listener(|m, _: &app::Refresh, _, cx| m.refresh(cx)))
        .on_action(|_: &app::CopySelection, window, cx| ui::select::copy(window, cx))
        .on_action(|_: &ShowWindow, window, _| window.activate_window())
        .when(keys, |d| {
            d.on_action(cx.listener(move |m, a: &app::GoBoard, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::GoProjects, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::GoGoals, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::GoBacklog, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::GoIdeas, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::GoDays, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::GoReleases, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::Search, w, cx| page(a, m, w, cx)))
                .on_action(cx.listener(move |m, a: &app::NewIdea, w, cx| page(a, m, w, cx)))
        })
        // ⌘N with New idea already open keeps the draft and puts focus back in Title.
        .when(!keys && matches!(m.modal, Some(ui::modals::Modal::NewIdea)), |d| d.on_action(cx.listener(|m, _: &app::NewIdea, w, cx| m.new_idea(w, cx))))
        .when(keys && m.can_go_back(), |d| d.on_action(cx.listener(move |m, a: &app::NavBack, w, cx| page(a, m, w, cx))))
        .when(keys && m.can_go_forward(), |d| d.on_action(cx.listener(move |m, a: &app::NavForward, w, cx| page(a, m, w, cx))))
        .when(keys && app::open_ref(m).is_some(), |d| d.on_action(cx.listener(move |m, a: &app::CopyRef, w, cx| page(a, m, w, cx))))
        .when(tc.focus, |d| {
            let r = tr.clone();
            d.on_action(cx.listener(move |m, _: &FocusInMidna, _, cx| m.focus_in_midna(format!("tasks/{r}/focus"), Some(r.clone()), cx)))
        })
        .when_some(tc.pr.clone(), |d, url| d.on_action(cx.listener(move |_, _: &OpenPr, _, cx| cx.open_url(&url))))
        .when(tc.start, |d| {
            let r = tr.clone();
            d.on_action(cx.listener(move |m, _: &StartTask, _, cx| {
                let r2 = r.clone();
                m.post(format!("tasks/{r}/start"), json!({"mode": "new"}), cx, move |m, _, cx| m.toast(format!("Starting {r2}"), false, cx));
            }))
        })
        .when_some(tc.stop, |d, force| {
            let r = tr.clone();
            d.on_action(cx.listener(move |m, _: &StopTask, _, cx| {
                let r2 = r.clone();
                m.post(format!("tasks/{r}/close-terminal"), json!({"force": force}), cx, move |m, _, cx| m.toast(format!("Closing {r2}’s terminal"), false, cx));
            }))
        })
        .when(tc.done, |d| {
            let r = tr.clone();
            d.on_action(cx.listener(move |m, _: &DoneTask, w, cx| m.set_modal(Some(ui::modals::Modal::finish(r.clone(), ui::modals::Ask::Done)), w, cx)))
        })
        .when(ic.dig, |d| {
            let r = ir.clone();
            d.on_action(cx.listener(move |m, _: &IdeaDig, _, cx| ui::idea::start(m, "dig", &r, cx)))
        })
        .when(ic.plan, |d| {
            let r = ir.clone();
            d.on_action(cx.listener(move |m, _: &IdeaPlan, _, cx| ui::idea::start(m, "plan", &r, cx)))
        })
        .when(ic.talk, |d| {
            let r = ir.clone();
            d.on_action(cx.listener(move |m, _: &IdeaTalk, _, cx| ui::idea::talk(m, &r, None, cx)))
        })
        .when(ic.close, |d| {
            let (r, r2) = (ir.clone(), ir.clone());
            d.on_action(cx.listener(move |m, _: &IdeaShelve, _, cx| ui::idea::close(m, "shelve", &r, cx)))
                .on_action(cx.listener(move |m, _: &IdeaDrop, _, cx| ui::idea::close(m, "drop", &r2, cx)))
        })
        .when(ic.reopen, |d| {
            let r = ir.clone();
            d.on_action(cx.listener(move |m, _: &IdeaReopen, _, cx| ui::idea::close(m, "reopen", &r, cx)))
        })
        .when(hooks, |d| d.on_action(cx.listener(|m, _: &InstallHooks, _, cx| m.install_hooks(cx))))
        .when(restart, |d| d.on_action(cx.listener(|m, _: &RestartDaemon, _, cx| m.restart_daemon(cx))))
}
