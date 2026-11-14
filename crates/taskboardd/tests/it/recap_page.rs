//! A recap's page: the spec Claude answers with, the checks it has to pass, the tries it gets and
//! the default page a recap falls back to.

use std::cell::RefCell;

use chrono::NaiveDate;
use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::iso;
use taskboardd::{days, fields, recap, recap_page};

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let repo = dir.path().join("webapp2");
    std::fs::create_dir_all(&repo).unwrap();
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp2", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    (dir, app)
}

fn post(app: &App, path: &str, body: Value) -> Result<Value, (u16, String)> {
    api::dispatch(app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
}

fn wed() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 7).unwrap()
}

/// A week of October 5, 2026 with one goal's three tasks merged; returns the recap id and the goal.
fn a_week(app: &App) -> (i64, i64) {
    let g = post(app, "/goals", json!({"name": "Ship login", "project": "webapp2"})).unwrap()["id"].as_i64().unwrap();
    for (i, title) in ["Login form", "Session cookie", "Write the copy"].iter().enumerate() {
        let t = post(app, "/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp2", "goal_id": g})).unwrap()["id"].as_i64().unwrap();
        let when = iso(days::start_of(wed()) + (10 + i as u32) as f64 * 3600.0);
        app.db.update("tasks", &json!(t), fields!["status" => "done", "finished_at" => when, "pr_num" => 5 + i as i64, "pr_state" => "MERGED"]).unwrap();
    }
    let id = recap::save(app, "weekly", wed()).unwrap();
    (id, g)
}

fn facts(app: &App, id: i64) -> Value {
    recap::get(app, id).unwrap()["facts"].clone()
}

fn good_spec(g: i64) -> Value {
    json!({
        "summary": format!("{{totals.tasks}} tasks finished, all for Ship login (G{g}) in webapp2."),
        "highlights": ["{totals.merged} merged."],
        "components": [
            {"component": "open", "why": "One goal made the week.", "props": []},
            {"component": "story", "why": "The week's goal.", "props": [{"name": "goal", "ref": format!("G{g}")}]},
            {"component": "kpis", "why": "The numbers.", "props": []},
            {"component": "fact", "why": "Worth a look.", "text": "{totals.tasks} tasks against {totals.merged} merged.",
             "props": [{"name": "a", "fact": "totals.tasks"}, {"name": "b", "fact": "totals.merged"}]},
        ],
    })
}

#[test]
fn a_spec_is_filled_from_the_facts_by_key_and_ref() {
    let (_dir, app) = new_board();
    let (id, g) = a_week(&app);
    let spec = good_spec(g);
    let ask = |_: String| Ok(spec.clone());
    let w = recap_page::compose(&app, id, Some(&ask)).unwrap();
    assert_eq!(w["source"], "ai");
    assert_eq!(w["attempts"], json!([]));
    let page = &w["page"];
    assert_eq!(page["summary"], format!("3 tasks finished, all for Ship login (G{g}) in webapp2."), "a project name's digits are its own");
    assert_eq!(page["highlights"], json!(["3 merged."]));
    let ids: Vec<&str> = page["components"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["open", "story", "kpis", "fact"], "in the spec's order");
    assert!(page["components"][1].get("why").is_none(), "why a component was picked stays in the spec, off the page");
    let story = &page["components"][1];
    assert_eq!(story["props"]["goal"]["ref"], format!("G{g}"));
    assert_eq!(story["props"]["goal"]["value"]["name"], "Ship login", "a ref is filled with what the facts say about it");
    let kpis = &page["components"][2];
    assert_eq!(kpis["props"]["totals"]["fact"], "totals", "a left-out prop takes its default key");
    assert_eq!(kpis["props"]["totals"]["value"]["tasks"], 3);
    assert!(kpis["props"].get("before").is_none(), "an optional prop with nothing behind it is dropped");
    assert_eq!(page["components"][3]["text"], "3 tasks against 3 merged.");
    assert_eq!(page["components"][3]["span"], "full");

    let r = recap::get(&app, id).unwrap();
    assert_eq!(r["writing_status"], "done");
    assert_eq!(r["writing"]["page"], *page, "kept on the recap");
}

#[test]
fn a_spec_is_rejected_for_an_unknown_component_fact_key_or_ref_or_a_number_it_wrote() {
    let (_dir, app) = new_board();
    let (id, g) = a_week(&app);
    let f = facts(&app, id);
    let reject = |edit: &dyn Fn(&mut Value)| {
        let mut s = good_spec(g);
        edit(&mut s);
        recap_page::check(&s, &f).unwrap_err()
    };
    if let Err(e) = recap_page::check(&good_spec(g), &f) {
        panic!("{e}");
    }
    assert!(reject(&|s| s["components"][2]["component"] = json!("pie")).contains("unknown component `pie`"));
    assert!(reject(&|s| s["components"][3]["props"][0]["fact"] = json!("totals.pies")).contains("unknown fact key `totals.pies`"));
    assert!(reject(&|s| s["components"][3]["props"][0]["fact"] = json!("before.tasks")).contains("unknown fact key"), "a key with nothing behind it");
    assert!(reject(&|s| s["components"][1]["props"][0]["ref"] = json!("G999")).contains("ref `G999` in `story` isn't in the facts"));
    assert!(reject(&|s| s["summary"] = json!("{totals.nope} tasks.")).contains("unknown fact key `totals.nope`"));
    assert!(reject(&|s| s["summary"] = json!("Three tasks in 2 days.")).contains("writes a number itself"));
    assert!(reject(&|s| s["highlights"] = json!(["T4 shipped."])).contains("ref `T4` isn't in the facts"));
    assert!(reject(&|s| s["components"][2]["component"] = json!("open")).contains("twice"));
    assert!(reject(&|s| s["components"][0]["component"] = json!("gcards")).contains("doesn't fit"), "fewer than three finished goals");
    assert!(reject(&|s| s["components"][0]["component"] = json!("slow")).contains("don't have agent time per task"));
    assert!(reject(&|s| s["components"][3]["props"][0]["fact"] = json!("totals")).contains("empty or too small"), "a's a number");
    assert!(reject(&|s| s["components"][3]["props"] = json!([{"name": "a", "fact": "totals.tasks"}])).contains("needs its `b` prop"));
    assert!(reject(&|s| s["components"][1]["props"][0] = json!({"name": "goal", "fact": "goals"})).contains("takes a ref"));
    assert!(reject(&|s| s["components"] = json!([])).contains("no components"));
    assert!(reject(&|s| s["components"][3]["props"][0] = json!({"name": "a", "ref": format!("G{g}")})).contains("takes a fact key"));

    let mut s = good_spec(g);
    s["components"][3]["props"][0] = json!({"name": "a", "ref": "totals.tasks"});
    s["summary"] = json!("{projects.count} project.");
    let page = recap_page::check(&s, &f).unwrap();
    assert_eq!(page["components"][3]["props"]["a"]["fact"], "totals.tasks", "a fact key sent as a ref is taken as the key");
    assert_eq!(page["summary"], "1 project.", "a list's count is a key of its own");
    assert!(reject(&|s| s["components"][0] = json!({"component": "train", "why": "", "props": []})).contains("doesn't fit"));

    let mut month = f.clone();
    month["kind"] = json!("monthly");
    let mut s = good_spec(g);
    s["components"][0] = json!({"component": "list", "why": "", "props": []});
    assert!(recap_page::check(&s, &month).unwrap_err().contains("left out of a month"));
}

#[test]
fn a_failed_or_rejected_pass_is_asked_again_with_the_reason_then_the_default_page_is_kept() {
    let (_dir, app) = new_board();
    let (id, g) = a_week(&app);
    let prompts = RefCell::new(Vec::<String>::new());
    let answers = RefCell::new(vec![
        Err("claude didn't answer in time".to_string()),
        Ok(json!({"summary": "x", "highlights": [], "components": [{"component": "pie", "why": "", "props": []}]})),
        Ok(json!({"summary": "x", "highlights": [], "components": [{"component": "story", "why": "", "props": [{"name": "goal", "ref": "G999"}]}]})),
        Ok(good_spec(g)),
    ]);
    let ask = |p: String| {
        prompts.borrow_mut().push(p);
        answers.borrow_mut().remove(0)
    };
    let w = recap_page::compose(&app, id, Some(&ask)).unwrap();
    assert_eq!(prompts.borrow().len(), recap_page::TRIES, "no more than the tries it's given");
    assert!(prompts.borrow()[2].contains("Your last page was turned down: unknown component `pie`"), "a retry says why");
    assert_eq!(w["source"], "default");
    assert_eq!(w["why"], "no spec from Claude passed the checks");
    let errors: Vec<&str> = w["attempts"].as_array().unwrap().iter().map(|a| a["error"].as_str().unwrap()).collect();
    assert_eq!(errors, ["claude didn't answer in time", "unknown component `pie`", "ref `G999` in `story` isn't in the facts"]);
    assert_eq!(w["attempts"][1]["spec"]["components"][0]["component"], "pie", "a turned-down spec is kept with its reason");
    assert_eq!(w["page"]["summary"], "3 tasks finished. 1 goal finished.", "the daemon's own summary; no PRs merged in this week's facts");
    assert_eq!(recap::get(&app, id).unwrap()["writing_status"], "done", "a recap always shows");

    let answers = RefCell::new(vec![Err("claude's answer had no result".to_string()), Ok(good_spec(g))]);
    let ask = |_: String| answers.borrow_mut().remove(0);
    let w = recap_page::compose(&app, id, Some(&ask)).unwrap();
    assert_eq!(w["source"], "ai", "a later try that passes is kept");
    assert_eq!(w["attempts"].as_array().unwrap().len(), 1);
}

#[test]
fn the_route_makes_a_named_period_and_without_claude_the_default_page_shows() {
    let (_dir, app) = new_board();
    a_week(&app);
    let r = post(&app, "/recaps", json!({"kind": "weekly", "day": "2026-10-08"})).unwrap();
    assert_eq!(r["first_day"], "2026-10-05");
    assert_eq!(r["writing_status"], "done", "Claude's off in tests, so the default page is written at once");
    assert_eq!(r["facts"]["totals"]["tasks"], 3, "the facts are kept first");
    let w = &r["writing"];
    assert_eq!(w["source"], "default");
    assert_eq!(w["why"], "Claude isn't available to the board");
    let ids: Vec<&str> = w["page"]["components"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["open", "kpis", "days", "list"], "only the default components that fit: no agent time, so no time breakdown");
    assert!(w["page"]["summary"].as_str().unwrap().starts_with("3 tasks finished"));

    let m = post(&app, "/recaps", json!({"kind": "monthly", "day": "2026-10"})).unwrap();
    assert_eq!((m["first_day"].as_str(), m["last_day"].as_str()), (Some("2026-10-01"), Some("2026-10-31")));
    let ids: Vec<&str> = m["writing"]["page"]["components"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(!ids.contains(&"list") && ids.contains(&"trend"), "a month leaves out the list and adds its weeks: {ids:?}");

    let empty = post(&app, "/recaps", json!({"kind": "weekly", "day": "2026-08-03"})).unwrap();
    assert_eq!(empty["writing"]["page"]["summary"], "Nothing finished this week.");
    let ids: Vec<&str> = empty["writing"]["page"]["components"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["open"], "an empty week still shows");

    assert_eq!(post(&app, "/recaps", json!({"kind": "weekly", "day": "Oct 8"})).unwrap_err().0, 400);
    assert_eq!(post(&app, "/recaps", json!({"kind": "daily", "day": "2026-10-08"})).unwrap_err().0, 400);
}

#[test]
fn the_prompt_lists_only_the_components_that_fit_and_the_schema_names_every_one() {
    let (_dir, app) = new_board();
    let (id, _) = a_week(&app);
    let f = facts(&app, id);
    let p = recap_page::prompt(&app, &f, None);
    assert!(p.contains("- story (Goal story"), "{p}");
    let fits = p.split("don't fit this period").next().unwrap();
    assert!(!fits.contains("- gcards ("), "fewer than three finished goals");
    assert!(!fits.contains("- slow ("));
    assert!(p.contains("totals.tasks: 3"));
    assert!(!p.contains("Your last page was turned down"));
    let ids = recap_page::schema()["properties"]["components"]["items"]["properties"]["component"]["enum"].as_array().unwrap().len();
    assert_eq!(ids, recap_page::CATALOG.len());
    assert_eq!(ids, 29);
}
