//! Handled for you, under Needs you: what the board took off the list by itself (`GET /handled`),
//! newest first and a page at a time, each with Pull back (`POST /handled/:id/pull-back`).
use super::*;
use crate::ui::paged::Paged;

pub const PAGE: usize = 50;
/// Lines left below the top of the view before the next older page is asked for.
const AHEAD: usize = 40;

#[derive(Default)]
pub struct Handled {
    pub held: Paged,
    /// The project filter the rows are for.
    pub project: String,
    pub open: bool,
    /// Rows whose Pull back is out, by record id.
    busy: HashSet<i64>,
}

impl Handled {
    /// What a refresh asks for: the first page, and once open, the rows held again (any can move on).
    pub fn recheck(&self) -> Vec<i64> {
        self.held.recheck(|_| self.open)
    }

    /// Land a refresh for `project`; rows for another project go first.
    pub fn land(&mut self, project: String, f: crate::ui::paged::Fresh) {
        if self.project != project {
            self.held = Paged::default();
            self.project = project;
        }
        self.held.land("items", f);
    }
}

pub fn query(project: &str) -> Vec<(&'static str, String)> {
    vec![("project", project.to_string())]
}

/// A row as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub id: i64,
    /// What the board did: (label, tone).
    pub pill: (&'static str, &'static str),
    /// "T12", "H4", "B9"
    pub r: String,
    pub title: String,
    /// The screener's reason or where the answer came from, else what the board did.
    pub why: String,
    /// "3:05 PM", "Yesterday 9:12 AM"
    pub when: String,
    pub can_pull: bool,
}

fn pill(v: &Value) -> (&'static str, &'static str) {
    match (s(v, "by"), s(v, "verdict")) {
        ("retry", _) => ("Retried", "accent"),
        (_, "agent") => ("To an agent", "goal"),
        (_, "answered") => ("Answered", "up"),
        _ => ("Handled", ""),
    }
}

pub fn line_vm(v: &Value) -> Line {
    let why = opt_s(v, "why").filter(|w| !w.is_empty()).unwrap_or(s(v, "text"));
    Line { id: i(v, "id"), pill: pill(v), r: s(v, "ref").to_string(), title: s(v, "title").to_string(), why: why.to_string(), when: fmt::day_time(s(v, "at")), can_pull: b(v, "can_pull") }
}

/// "Handled for you 4"
pub fn label(total: usize) -> String {
    format!("Handled for you {total}")
}

/// What clicking a row opens: the miss's backlog item, else the task.
fn opens(r: &str) -> Act {
    if r.starts_with('B') { Act::OpenIssue(r.to_string()) } else { Act::OpenTask(r.to_string()) }
}

pub fn head(t: &Theme, total: usize, open: bool, chevron: impl IntoElement, cx: &mut Context<MainWindow>) -> Div {
    div().pt(px(12.)).pb(px(4.)).child(
        crate::ui::focus::stop(t, div().id("home-handled-line"))
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.5))
            .text_color(t.muted)
            .cursor_pointer()
            .on_click(cx.listener(move |m, _, _, cx| {
                m.home.handled.open = !open;
                cx.notify();
            }))
            .child(chevron)
            .child(label(total)),
    )
}

pub fn item(m: &MainWindow, t: &Theme, ix: usize, cx: &mut Context<MainWindow>) -> Div {
    let Some(v) = m.home.handled.held.rows.get(ix) else { return div() };
    let x = line_vm(v);
    let busy = m.home.handled.busy.contains(&x.id);
    let pull = x.can_pull.then(|| {
        let b = kit::btn_small(t, SharedString::from(format!("home-handled-pull-{}", x.id)), if busy { "Sending…" } else { "Pull back" })
            .flex_none()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if busy || paused(m) {
            kit::disabled(b)
        } else {
            let id = x.id;
            b.on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                pull_back(m, id, cx);
            }))
        }
    });
    let act = opens(&x.r);
    let head = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(kit::tone_pill(t, x.pill.1, x.pill.0).flex_none())
        .child(div().flex_none().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(x.r.clone()))
        .child(div().flex_1().min_w_0().truncate().text_size(px(14.)).text_color(t.text).child(x.title.clone()))
        .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(x.when.clone()))
        .children(pull);
    let card = crate::ui::focus::stop(t, div().id(SharedString::from(format!("home-handled-{}", x.id))))
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(14.))
        .py(px(9.))
        .rounded(px(10.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .cursor_pointer()
        .on_click(cx.listener(move |m, _, _, cx| run_act(m, &act, cx)))
        .child(head)
        .when(!x.why.is_empty(), |d| d.child(div().text_size(px(12.5)).text_color(t.muted).child(x.why.clone())));
    div().pb(px(6.)).child(card)
}

/// Pull back: the item goes back on Needs you, and its row goes from here at once.
fn pull_back(m: &mut MainWindow, id: i64, cx: &mut Context<MainWindow>) {
    if !m.home.handled.busy.insert(id) {
        return;
    }
    m.post_or(
        format!("handled/{id}/pull-back"),
        json!({}),
        cx,
        move |m, v, cx| {
            let h = &mut m.home.handled;
            h.busy.remove(&id);
            h.held.drop_row(id);
            sent(m, s(&v, "said").to_string(), cx);
        },
        move |m, e, cx| {
            m.home.handled.busy.remove(&id);
            m.toast(e.message, true, cx);
        },
    );
    cx.notify();
}

/// Ask for the next older page once the open list nears its end.
pub fn load_older(m: &mut MainWindow, lines: usize, top: usize, cx: &mut Context<MainWindow>) {
    let h = &mut m.home.handled;
    let Some(before) = h.held.next.clone() else { return };
    if !h.open || h.held.loading || lines > top + AHEAD {
        return;
    }
    h.held.loading = true;
    let backend = m.backend.clone();
    let mut query = query(&h.project);
    query.extend([("limit", PAGE.to_string()), ("before", before.clone())]);
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get("handled", &query).map_err(|e| e.message) }).await;
        let _ = this.update(cx, |m, cx| {
            m.home.handled.held.land_older("items", &before, got);
            cx.notify();
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;

    #[test]
    fn a_row_shows_what_the_board_did_and_why() {
        let v = json!({"id": 7, "at": "2026-11-23T15:05:00Z", "by": "recheck", "item": "human", "verdict": "agent",
                       "text": "Gave H4 to an agent", "why": "The Amplitude MCP reads it.", "ref": "H4", "title": "Check retention", "can_pull": true});
        let x = line_vm(&v);
        assert_eq!((x.pill, x.r.as_str(), x.title.as_str(), x.why.as_str(), x.can_pull), (("To an agent", "goal"), "H4", "Check retention", "The Amplitude MCP reads it.", true));
        assert!(x.when.ends_with("PM"), "12-hour: {}", x.when);
        let answered = line_vm(&json!({"by": "recheck", "verdict": "answered", "why": null, "text": "Answered T3 from rules.md"}));
        assert_eq!((answered.pill.0, answered.why.as_str()), ("Answered", "Answered T3 from rules.md"));
        assert_eq!(line_vm(&json!({"by": "retry", "verdict": "retried"})).pill.0, "Retried");
        assert!(matches!(opens("B9"), Act::OpenIssue(_)) && matches!(opens("T3"), Act::OpenTask(_)));
    }

    #[gpui_kit::test]
    fn pull_back_puts_a_question_back_on_needs_you_and_takes_its_row_off(cx: &mut gpui_kit::TestAppContext) {
        use crate::parity::{settle, window};
        use taskboardd::util::RowExt;
        let (w, rec) = window(cx);
        let app = rec.board().app();
        let t = app.db.q1("SELECT * FROM tasks WHERE status = 'working' ORDER BY id LIMIT 1", vec![]).unwrap().unwrap().id();
        let data = json!({"item": "question", "verdict": "answered", "was": "Which port does it use?", "answer": "8080", "source": "rules.md"});
        let id = taskboardd::handled::record(app, t, "recheck", &format!("Answered T{t} from rules.md"), data).unwrap();
        w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, cx| {
            let h = &m.home.handled;
            assert_eq!((h.held.total, h.held.rows.len()), (1, 1));
            assert_eq!(line_vm(&h.held.rows[0]).why, "From rules.md");
            pull_back(m, id, cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last(&format!("handled/{id}/pull-back")), Some(json!({})));
        w.update(cx, |m, _, _| {
            assert!(m.home.handled.held.rows.is_empty(), "its row goes");
            assert_eq!(m.toasts.last().map(|t| t.text.to_string()), Some(format!("T{t}'s question is back on Needs you")));
        })
        .unwrap();
        let task = app.db.q1("SELECT * FROM tasks WHERE id = ?", vec![json!(t)]).unwrap().unwrap();
        assert_eq!((task.s("status"), task.s("question")), (Some("needs"), Some("Which port does it use?")));
    }

    #[test]
    fn rows_for_another_project_go_when_its_list_lands() {
        let mut h = Handled { project: "webapp".into(), ..Handled::default() };
        h.held.rows = Rc::new(vec![json!({"id": 3})]);
        let fresh = |rows: Value| crate::ui::paged::Fresh::for_tests(json!({"items": rows, "total": 1, "next": null}));
        h.land("api".into(), fresh(json!([{"id": 9}])));
        assert_eq!((h.project.as_str(), h.held.rows.len()), ("api", 1));
        assert_eq!(h.held.rows[0]["id"], 9);
    }
}
