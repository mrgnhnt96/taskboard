//! A recap's page: a headless Claude reads the recap's saved facts and the catalog of approved
//! components, with each one's when-to-use rule, and answers with a page spec: the components in
//! order, each with its props, plus a summary and up to four highlights. It never writes a number.
//! Each prop points at a fact by key (or at a goal, task, release or lesson by ref), and the summary
//! and highlights hold `{key}` placeholders. The daemon checks the spec and fills in the values.
//!
//! A spec is rejected when it names a component that isn't in the catalog or doesn't fit this
//! period's facts, a fact key that isn't in the facts, or a ref that isn't in them, or when its text
//! has a digit of its own. A rejected spec or a failed pass is asked again, up to [`TRIES`] times
//! in all. After that, or when Claude isn't available (`triage::ai_on`), the recap gets the
//! [`default_page`], so a recap always shows.
//!
//! The result goes in `recaps.writing`: `source` (`ai` or `default`), why the default was used,
//! each failed attempt with its spec, the spec that passed, and the `page` the app draws. `writing_status`
//! is `writing` while the pass is queued and `done` once a page is kept, whichever made it.

use std::collections::BTreeSet;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::app::App;
use crate::util::*;
use crate::{p, review, triage};

/// The shape of `writing` this module keeps.
pub const PAGE_VERSION: i64 = 1;
/// Passes at a spec before the default page is used.
pub const TRIES: usize = 3;
const BUDGET_USD: &str = "1.50";
const TIMEOUT_SECS: u64 = 300;
const MAX_HIGHLIGHTS: usize = 4;
/// Tasks listed per goal in the facts the prompt shows.
const PROMPT_TASKS: usize = 5;

// ------------------------------------------------------------------ the catalog

#[derive(Clone, Copy, PartialEq, Debug)]
enum Need {
    /// Dropped when it doesn't resolve.
    Opt,
    /// Something there: a non-zero number, a non-empty string or list, an object with one of those.
    Some,
    /// A number at least this big, or a list at least this long.
    Min(f64),
    /// Any number.
    Number,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Points {
    /// A fact key; the default key when the spec gives none.
    Fact(Option<&'static str>),
    /// A ref starting with this letter.
    Ref(char),
}

struct Prop {
    name: &'static str,
    points: Points,
    need: Need,
}

const fn fact(name: &'static str, key: &'static str, need: Need) -> Prop {
    Prop { name, points: Points::Fact(Some(key)), need }
}

const fn pick(name: &'static str, need: Need) -> Prop {
    Prop { name, points: Points::Fact(None), need }
}

const fn goal(name: &'static str) -> Prop {
    Prop { name, points: Points::Ref('G'), need: Need::Some }
}

pub struct Component {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub span: &'static str,
    rule: &'static str,
    props: &'static [Prop],
    /// Left out of a month's recap.
    month_skip: bool,
    /// What the facts don't have yet, when they can't fill it.
    missing: Option<&'static str>,
    /// Takes a sentence of its own (`text`).
    text: bool,
}

const fn c(id: &'static str, name: &'static str, group: &'static str, span: &'static str, rule: &'static str, props: &'static [Prop]) -> Component {
    Component { id, name, group, span, rule, props, month_skip: false, missing: None, text: false }
}

impl Component {
    const fn no_month(mut self) -> Self {
        self.month_skip = true;
        self
    }
    const fn missing(mut self, what: &'static str) -> Self {
        self.missing = Some(what);
        self
    }
    const fn text(mut self) -> Self {
        self.text = true;
        self
    }
}

use Need::{Min, Number, Opt, Some as Any};

pub static CATALOG: &[Component] = &[
    c("hero", "Hero stat", "Headline", "half", "Opens an ordinary period with the task count. Never with heroR. Skip when tasks are under half of the previous period.",
      &[fact("lead", "totals.tasks", Any), fact("before", "before.tasks", Opt), fact("projects", "projects", Opt)]),
    c("heroR", "Hero stat, releases", "Headline", "half", "Opens a release-heavy period with the release count. Never with hero.",
      &[fact("lead", "totals.releases", Any), fact("before", "before.releases", Opt), fact("releases", "releases", Any)]),
    c("kpis", "KPI row", "Headline", "full", "Default summary under the opening: tasks, PRs, releases, finished goals, agent time, against the previous period. Once per page; don't put it beside a hero stat that leads with the same number.",
      &[fact("totals", "totals", Any), fact("before", "before", Opt), fact("days", "days", Opt)]),
    c("open", "Opening line", "Headline", "full", "The page's summary as its opening sentence. Opens a quiet period and a one-goal period. Always fits.", &[]),
    c("gcards", "Shipped-goal cards", "Shipped", "full", "Use with 3 or more finished goals. Top 6 by estimated human time.",
      &[fact("goals", "goals", Min(3.0))]),
    c("story", "Goal story", "Shipped", "half", "The one goal that made the period: lead with it when it is 40% or more of the estimated human time or the tasks.",
      &[goal("goal")]),
    c("spot", "Biggest-win spotlight", "Shipped", "full", "The goal that changed the most; a first release beats a larger estimate. A month shows the runners-up too.",
      &[goal("goal"), fact("goals", "goals", Opt)]),
    c("timeline", "Release timeline", "Shipped", "full", "Lead with it at 15 or more releases in a week or 40 in a month. Never with train.",
      &[fact("releases", "releases", Any)]),
    c("train", "Release train", "Shipped", "half", "Compact alternative to the timeline, for releases in 2 or more projects. Never with timeline.",
      &[fact("releases", "releases", Min(2.0))]),
    c("list", "Everything shipped", "Shipped", "full", "Goals, then loose tasks by project.",
      &[fact("tasks", "tasks", Any)]).no_month(),
    c("time", "Time breakdown", "Time", "half", "Default for where the time went: command time by category, agent time by project. Don't lead with Other.",
      &[fact("categories", "agent.categories", Any), fact("projects", "agent.projects", Opt)]),
    c("vs", "Agent vs estimated human", "Time", "half", "Estimated developer time against agent time. Needs 10 or more estimated hours; say \"estimated\".",
      &[fact("human", "totals.est_human_min", Min(600.0)), fact("agent", "totals.est_agent_min", Opt), fact("estimated", "totals.estimated", Opt)]),
    c("slow", "Slowest tasks", "Time", "half", "Top 5 tasks by agent time.", &[]).missing("agent time per task"),
    c("wait", "Waiting on you", "Time", "half", "Time tasks waited on the owner. Lead a quiet period with it when the waits explain it. Needs an hour or more.",
      &[fact("minutes", "waiting.minutes", Min(60.0)), fact("reasons", "waiting.reasons", Opt), fact("tasks", "waiting.tasks", Opt)]),
    c("usage", "Usage meter", "Time", "full", "Plan usage in percent; prefers the 7-day window.",
      &[fact("seven_day", "usage.seven_day", Any), fact("five_hour", "usage.five_hour", Opt)]),
    c("heat", "Busiest hours", "Time", "full", "Finished tasks by weekday and hour, at 50 or more tasks.", &[]).missing("finished tasks by weekday and hour"),
    c("days", "Day-by-day strip", "Trends", "full", "The shape of the period day by day; leads a quiet week.",
      &[fact("days", "days", Any), fact("tasks", "totals.tasks", Any)]),
    c("ba", "Before and after", "Trends", "half", "This period's numbers against the previous one's, when both had work.",
      &[fact("this", "totals", Any), fact("before", "before", Any)]),
    c("trend", "Week-by-week trend", "Trends", "half", "A month's own weeks, at 3 or more.",
      &[fact("weeks", "weeks", Min(3.0))]),
    c("lines", "Lines changed", "Trends", "full", "Lines added and removed.",
      &[fact("added", "totals.added", Any), fact("removed", "totals.removed", Opt)]),
    c("board", "Project leaderboard", "Trends", "half", "Projects ranked by tasks, for 2 or more projects.",
      &[fact("projects", "projects", Min(2.0))]),
    c("bump", "Project rank shift", "Trends", "half", "When the top project changed.", &[]).missing("the previous period's project ranks").no_month(),
    c("tries", "Merged first try", "Quality", "half", "PRs merged without a retry.", &[]).missing("retries recorded for every PR").no_month(),
    c("plan", "Plan-quality scorecard", "Quality", "full", "A month's plan quality against the month before.",
      &[fact("this", "quality.this", Any), fact("before", "quality.before", Opt)]),
    c("lessons", "Lessons kept", "Quality", "half", "Lessons kept in the period, at most 5.",
      &[fact("lessons", "lessons", Any)]),
    c("issues", "Found along the way", "Quality", "half", "Problems agents found and filed, when 10 or more were.",
      &[fact("found", "issues.found", Min(10.0)), fact("kinds", "issues.kinds", Opt)]),
    c("fact", "Surprising fact", "Callouts", "full", "One per page: the largest change the headline doesn't explain, as a sentence with both numbers as placeholders. Props a and b are the two numbers' keys.",
      &[pick("a", Number), pick("b", Number)]).text(),
    c("quiet", "Quiet-period note", "Callouts", "half", "States plainly, in its sentence, that the period was quiet and why. Only on a quiet period.",
      &[fact("days", "days", Any)]).text(),
    c("first", "Milestones", "Callouts", "half", "Up to 3 firsts, such as a project's first release, in its sentence.",
      &[fact("releases", "releases", Any)]).text(),
];

const NEVER_BOTH: &[(&str, &str)] = &[("hero", "heroR"), ("timeline", "train")];

/// The page used when there's no spec from Claude: whichever of these fit, in this order.
const DEFAULT: &[&str] = &["open", "kpis", "gcards", "days", "trend", "time", "list", "board", "timeline", "lessons"];

fn component(id: &str) -> Option<&'static Component> {
    CATALOG.iter().find(|c| c.id == id)
}

// ------------------------------------------------------------------ facts, keys and refs

/// Top-level facts a key can't start with: they're the page's own frame.
const NOT_KEYS: &[&str] = &["version", "kind", "period", "labels"];
const REF_FIELDS: &[&str] = &["task", "release", "lesson", "goal"];

/// Every fact key: the dotted path to each non-null value through objects (never into lists), and
/// each list's length as `<list>.count`.
fn keys(facts: &Value) -> BTreeSet<String> {
    fn walk(v: &Value, path: String, out: &mut BTreeSet<String>) {
        if v.is_null() {
            return;
        }
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    walk(x, if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out);
                }
            }
            Value::Array(_) => {
                out.insert(format!("{path}.count"));
            }
            _ => {}
        }
        if !path.is_empty() {
            out.insert(path);
        }
    }
    let mut out = BTreeSet::new();
    if let Value::Object(m) = facts {
        for (k, v) in m.iter().filter(|(k, _)| !NOT_KEYS.contains(&k.as_str())) {
            walk(v, k.clone(), &mut out);
        }
    }
    out
}

fn at<'a>(facts: &'a Value, key: &str) -> &'a Value {
    key.split('.').fold(facts, |v, k| &v[k])
}

/// The fact behind `key`, a list's `.count` included.
fn value(facts: &Value, key: &str) -> Value {
    if let Some(Value::Array(a)) = key.strip_suffix(".count").map(|b| at(facts, b)) {
        return json!(a.len());
    }
    at(facts, key).clone()
}

/// The ref an object stands for: its first of task, release, lesson and goal.
fn own_ref(m: &Map<String, Value>) -> Option<&str> {
    REF_FIELDS.iter().find_map(|f| m.get(*f)).and_then(Value::as_str)
}

/// Every goal, task, release and lesson ref in the facts, with the first object that stands for it.
fn refs(facts: &Value) -> Vec<(String, Value)> {
    fn walk(v: &Value, out: &mut Vec<(String, Value)>) {
        match v {
            Value::Object(m) => {
                if let Some(r) = own_ref(m) {
                    if !out.iter().any(|(x, _)| x == r) {
                        out.push((r.to_string(), v.clone()));
                    }
                }
                m.values().for_each(|x| walk(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(facts, &mut out);
    out
}

/// Names, titles and refs from the facts, longest first: text may quote them, digits and all.
fn known_words(facts: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if let (true, Some(s)) = (matches!(k.as_str(), "name" | "title" | "project" | "task" | "goal" | "release" | "lesson"), x.as_str()) {
                        out.insert(s.to_string());
                    }
                    walk(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut set = BTreeSet::new();
    walk(facts, &mut set);
    let mut out: Vec<String> = set.into_iter().filter(|s| !s.trim().is_empty()).collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    out
}

fn some(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => m.values().any(some),
        Value::Bool(b) => *b,
        Value::Null => false,
    }
}

fn meets(v: &Value, need: Need) -> bool {
    match need {
        Need::Opt => true,
        Need::Some => some(v),
        Need::Number => v.is_number(),
        Need::Min(n) => match v {
            Value::Number(x) => x.as_f64().is_some_and(|x| x >= n),
            Value::Array(a) => a.len() as f64 >= n,
            _ => false,
        },
    }
}

/// Why `c` can't go on a page for these facts, if it can't.
fn unfit(c: &Component, facts: &Value) -> Option<String> {
    if let Some(m) = c.missing {
        return Some(format!("the facts don't have {m} yet"));
    }
    if c.month_skip && facts["kind"] == "monthly" {
        return Some("it's left out of a month".into());
    }
    c.props.iter().find_map(|p| match p.points {
        Points::Fact(Some(k)) if !meets(&value(facts, k), p.need) => Some(format!("`{k}` is empty or too small here")),
        _ => None,
    })
}

// ------------------------------------------------------------------ checking a spec

/// The page a spec makes from `facts`, or why it's rejected.
pub fn check(spec: &Value, facts: &Value) -> std::result::Result<Value, String> {
    let keys = keys(facts);
    let refs = refs(facts);
    let words = known_words(facts);
    let list = spec["components"].as_array().filter(|a| !a.is_empty()).ok_or("the spec has no components")?;
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for item in list {
        let id = item["component"].as_str().unwrap_or_default();
        let c = component(id).ok_or_else(|| format!("unknown component `{id}`"))?;
        if seen.contains(&id) {
            return Err(format!("`{id}` is on the page twice"));
        }
        if let Some((a, b)) = NEVER_BOTH.iter().find(|(a, b)| (*a == id && seen.contains(b)) || (*b == id && seen.contains(a))) {
            return Err(format!("`{a}` and `{b}` can't both be on a page"));
        }
        seen.push(id);
        if let Some(why) = unfit(c, facts) {
            return Err(format!("`{id}` doesn't fit this period: {why}"));
        }
        let given = item["props"].as_array().cloned().unwrap_or_default();
        for g in &given {
            let name = g["name"].as_str().unwrap_or_default();
            if !c.props.iter().any(|p| p.name == name) {
                return Err(format!("`{id}` has no prop `{name}`"));
            }
        }
        let mut props = Map::new();
        for p in c.props {
            let g = given.iter().find(|g| g["name"] == p.name);
            let (fact_key, ref_key) = (g.and_then(|g| g["fact"].as_str()).filter(|s| !s.is_empty()), g.and_then(|g| g["ref"].as_str()).filter(|s| !s.is_empty()));
            let filled = match p.points {
                Points::Fact(default) => {
                    let fact_key = match (fact_key, ref_key) {
                        (None, Some(r)) if keys.contains(r) => Some(r),
                        (_, Some(_)) => return Err(format!("`{id}`'s `{}` takes a fact key: put it in `fact`, not `ref`", p.name)),
                        (k, None) => k,
                    };
                    match fact_key.or(default) {
                        Some(k) if keys.contains(k) => Some(json!({"fact": k, "value": value(facts, k)})),
                        Some(k) if fact_key.is_some() => return Err(format!("unknown fact key `{k}` in `{id}`'s `{}`", p.name)),
                        _ => None,
                    }
                }
                Points::Ref(letter) => {
                    if fact_key.is_some() {
                        return Err(format!("`{id}`'s `{}` takes a ref, not a fact key", p.name));
                    }
                    match ref_key {
                        Some(r) if !r.starts_with(letter) => return Err(format!("`{id}`'s `{}` takes a {letter} ref, not `{r}`", p.name)),
                        Some(r) => match refs.iter().find(|(x, _)| x == r) {
                            Some((_, v)) => Some(json!({"ref": r, "value": v})),
                            None => return Err(format!("ref `{r}` in `{id}` isn't in the facts")),
                        },
                        None => None,
                    }
                }
            };
            match filled {
                Some(v) if meets(&v["value"], p.need) => {
                    props.insert(p.name.into(), v);
                }
                Some(v) if p.need != Need::Opt => {
                    return Err(format!("`{id}`'s `{}` ({}) is empty or too small", p.name, v.get("fact").or(v.get("ref")).unwrap_or(&Value::Null)));
                }
                None if p.need != Need::Opt => return Err(format!("`{id}` needs its `{}` prop", p.name)),
                _ => {}
            }
        }
        let mut entry = json!({"id": c.id, "name": c.name, "group": c.group, "span": c.span, "why": item["why"].as_str().unwrap_or_default(), "props": props});
        let text = item["text"].as_str().unwrap_or_default().trim();
        if c.text {
            if text.is_empty() {
                return Err(format!("`{id}` needs its sentence (`text`)"));
            }
            entry["text"] = json!(fill(text, facts, &keys, &words).map_err(|e| format!("`{id}`'s text: {e}"))?);
        }
        out.push(entry);
    }
    let summary = spec["summary"].as_str().unwrap_or_default().trim();
    if summary.is_empty() {
        return Err("the summary is empty".into());
    }
    let summary = fill(summary, facts, &keys, &words).map_err(|e| format!("the summary: {e}"))?;
    let highlights = spec["highlights"].as_array().cloned().unwrap_or_default();
    if highlights.len() > MAX_HIGHLIGHTS {
        return Err(format!("{} highlights; at most {MAX_HIGHLIGHTS}", highlights.len()));
    }
    let highlights = highlights
        .iter()
        .filter_map(|h| h.as_str().map(str::trim).filter(|h| !h.is_empty()))
        .map(|h| fill(h, facts, &keys, &words).map_err(|e| format!("a highlight: {e}")))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(json!({"summary": summary, "highlights": highlights, "components": out}))
}

static PLACEHOLDER: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{([A-Za-z_][A-Za-z0-9_.]*)\}").unwrap());
static REF: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[TGBJQMLRIH][0-9]+\b").unwrap());

/// `text` with each `{key}` placeholder replaced by its fact, or why it can't be: a key that isn't
/// a number or word in the facts, a ref that isn't in them, or a digit the facts didn't give.
fn fill(text: &str, facts: &Value, keys: &BTreeSet<String>, words: &[String]) -> std::result::Result<String, String> {
    for cap in PLACEHOLDER.captures_iter(text) {
        let k = &cap[1];
        if !keys.contains(k) {
            return Err(format!("unknown fact key `{k}`"));
        }
        let v = value(facts, k);
        if !(v.is_number() || v.is_string()) {
            return Err(format!("`{k}` isn't a single number or word"));
        }
    }
    let mut rest = PLACEHOLDER.replace_all(text, " ").to_string();
    for w in words {
        rest = rest.replace(w.as_str(), " ");
    }
    if let Some(m) = REF.find(&rest) {
        return Err(format!("ref `{}` isn't in the facts", m.as_str()));
    }
    if let Some(i) = rest.find(|c: char| c.is_ascii_digit()) {
        let start = rest[..i].char_indices().rev().nth(19).map_or(0, |(j, _)| j);
        let around: String = rest[start..].chars().take(40).collect();
        return Err(format!("it writes a number itself (\"{}\"); use a {{fact}} placeholder", around.trim()));
    }
    Ok(PLACEHOLDER.replace_all(text, |cap: &regex::Captures| show(&cap[1], &value(facts, &cap[1]))).to_string())
}

/// A fact as the page writes it: minutes as hours, usage in percent, big numbers grouped.
fn show(key: &str, v: &Value) -> String {
    let Some(n) = v.as_f64() else { return v.as_str().unwrap_or_default().to_string() };
    if key.ends_with("_min") || key.ends_with(".minutes") {
        return review::hours_text(n * 60.0);
    }
    let s = if n.fract() == 0.0 { grouped(n as i64) } else { format!("{:.1}", n) };
    if key.starts_with("usage.") && !key.ends_with("samples") {
        format!("{s}%")
    } else {
        s
    }
}

fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

// ------------------------------------------------------------------ the default page

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// The daemon's own summary of the facts.
fn default_summary(facts: &Value) -> String {
    let t = &facts["totals"];
    let n = |k: &str| t[k].as_i64().unwrap_or(0);
    let span = if facts["kind"] == "monthly" { "month" } else { "week" };
    if n("tasks") == 0 {
        return format!("Nothing finished this {span}.");
    }
    let projects = facts["projects"].as_array().map_or(0, |p| p.iter().filter(|p| p["tasks"].as_i64().unwrap_or(0) > 0).count()) as i64;
    let mut s = format!("{} finished", plural(n("tasks"), "task", "tasks"));
    if projects > 1 {
        s += &format!(" across {}", plural(projects, "project", "projects"));
    }
    s += &match n("prs") {
        0 => ".".to_string(),
        prs => format!(", with {} merged.", plural(prs, "PR", "PRs")),
    };
    if n("goals_finished") > 0 {
        s += &format!(" {} finished.", plural(n("goals_finished"), "goal", "goals"));
    }
    if n("releases") > 0 {
        s += &format!(" {} cut.", plural(n("releases"), "release", "releases"));
    }
    s
}

/// The fixed page: the [`DEFAULT`] components that fit these facts, under the daemon's summary.
pub fn default_page(facts: &Value) -> Value {
    let mut components = Vec::new();
    for id in DEFAULT {
        let one = json!({"summary": "-", "components": [{"component": id, "why": "The default page."}]});
        if let Ok(p) = check(&one, facts) {
            components.extend(p["components"].as_array().cloned().unwrap_or_default());
        }
    }
    json!({"summary": default_summary(facts), "highlights": [], "components": components})
}

// ------------------------------------------------------------------ asking Claude

/// The answer's JSON schema.
pub fn schema() -> Value {
    let ids: Vec<&str> = CATALOG.iter().map(|c| c.id).collect();
    json!({
        "type": "object", "additionalProperties": false, "required": ["summary", "highlights", "components"],
        "properties": {
            "summary": {"type": "string"},
            "highlights": {"type": "array", "maxItems": MAX_HIGHLIGHTS, "items": {"type": "string"}},
            "components": {"type": "array", "minItems": 1, "items": {
                "type": "object", "additionalProperties": false, "required": ["component", "why", "props"],
                "properties": {
                    "component": {"type": "string", "enum": ids},
                    "why": {"type": "string"},
                    "text": {"type": "string"},
                    "props": {"type": "array", "items": {
                        "type": "object", "additionalProperties": false, "required": ["name"],
                        "properties": {"name": {"type": "string"}, "fact": {"type": "string"}, "ref": {"type": "string"}},
                    }},
                },
            }},
        },
    })
}

/// The facts as the prompt shows them: task lists cut short, task summaries left out.
fn prompt_facts(facts: &Value) -> Value {
    let mut f = facts.clone();
    if let Some(o) = f.as_object_mut() {
        o.remove("labels");
    }
    fn trim_tasks(list: &mut Value, keep: usize) {
        if let Some(a) = list.as_array_mut() {
            a.truncate(keep);
            for t in a {
                if let Some(o) = t.as_object_mut() {
                    o.remove("summary");
                    o.remove("finished_at");
                }
            }
        }
    }
    for g in f["tasks"]["by_goal"].as_array_mut().into_iter().flatten() {
        let n = g["tasks"].as_array().map_or(0, Vec::len);
        trim_tasks(&mut g["tasks"], PROMPT_TASKS);
        g["count"] = json!(n);
    }
    for g in f["tasks"]["loose"].as_array_mut().into_iter().flatten() {
        trim_tasks(&mut g["tasks"], PROMPT_TASKS);
    }
    for r in f["releases"].as_array_mut().into_iter().flatten() {
        trim_tasks(&mut r["tasks"], 3);
    }
    for w in ["five_hour", "seven_day"] {
        if let Some(o) = f["usage"][w].as_object_mut() {
            o.remove("by_day");
        }
    }
    f
}

/// What stands out about the period, worked out so Claude doesn't have to.
fn signals(facts: &Value) -> Vec<String> {
    let t = &facts["totals"];
    let n = |v: &Value| v.as_f64().unwrap_or(0.0);
    let mut out = Vec::new();
    match facts["before"].is_null() {
        true => out.push("There's no previous period to compare with (no data before it).".to_string()),
        false => {
            let (now, then) = (n(&t["tasks"]), n(&facts["before"]["tasks"]));
            if then > 0.0 {
                out.push(format!("Tasks are {:.0}% of the previous period's.", now / then * 100.0));
            }
        }
    }
    out.push(format!("Releases: {}.", n(&t["releases"])));
    let goals = facts["goals"].as_array().cloned().unwrap_or_default();
    out.push(format!("Finished goals: {}.", goals.len()));
    if let Some(top) = goals.iter().max_by(|a, b| n(&a["est_human_min"]).total_cmp(&n(&b["est_human_min"]))) {
        let est = n(&t["est_human_min"]);
        let tasks = n(&t["tasks"]);
        out.push(format!(
            "The top goal by estimated human time, {} ({}), is {:.0}% of the period's estimated human time and {:.0}% of its tasks.",
            top["goal"].as_str().unwrap_or_default(),
            top["name"].as_str().unwrap_or_default(),
            if est > 0.0 { n(&top["est_human_min"]) / est * 100.0 } else { 0.0 },
            if tasks > 0.0 { n(&top["tasks"]) / tasks * 100.0 } else { 0.0 },
        ));
    }
    let projects = facts["projects"].as_array().map_or(0, Vec::len);
    out.push(format!("Projects with activity: {projects}."));
    if n(&t["agent_min"]) > 0.0 && n(&t["command_min"]) == 0.0 {
        out.push("No command time was recorded, so the time by category is empty.".into());
    }
    out
}

/// When the board started keeping the data some components need.
fn data_notes(app: &App) -> Vec<String> {
    let first = |sql: &str| app.db.q1(sql, p![]).ok().flatten().and_then(|r| r.s("at").map(|s| s.chars().take(10).collect::<String>()));
    let mut out = Vec::new();
    if let Some(d) = first("SELECT MIN(started_at) AS at FROM spans") {
        out.push(format!("Command timings start {d}; before that the time by category is empty."));
    }
    if let Some(d) = first("SELECT MIN(at) AS at FROM usage_samples") {
        out.push(format!("Usage samples start {d}."));
    }
    if let Some(d) = first("SELECT MIN(cut_at) AS at FROM releases") {
        out.push(format!("Releases are recorded from {d}; a release marked first is the first the board saw for its project, a true first only when its name is v0.1.0 or lower."));
    }
    out.push("Estimates cover only the tasks that have one (totals.estimated says how many).".into());
    out.push("Waits add up across tasks, so they can pass the hours in the period.".into());
    out
}

fn catalog_text(facts: &Value) -> String {
    let mut fits = String::new();
    let mut not = String::new();
    for c in CATALOG {
        match unfit(c, facts) {
            Some(why) => not += &format!("- {} ({}): {why}\n", c.id, c.name),
            None => {
                let props: Vec<String> = c
                    .props
                    .iter()
                    .map(|p| match p.points {
                        Points::Fact(Some(k)) => format!("{} = fact {k} by default{}", p.name, if p.need == Need::Opt { ", optional" } else { "" }),
                        Points::Fact(None) => format!("{} = a numeric fact key you choose (required)", p.name),
                        Points::Ref(l) => format!("{} = a {l} ref from the facts (required)", p.name),
                    })
                    .collect();
                fits += &format!(
                    "- {} ({}, {}, {} width): {}{}{}\n",
                    c.id,
                    c.name,
                    c.group,
                    c.span,
                    c.rule,
                    if props.is_empty() { String::new() } else { format!(" Props: {}.", props.join("; ")) },
                    if c.text { " Needs `text`: its own sentence." } else { "" },
                );
            }
        }
    }
    format!("Components that fit this period:\n{fits}\nComponents that don't fit this period (never use them):\n{not}")
}

fn key_list(facts: &Value) -> String {
    keys(facts)
        .iter()
        .map(|k| match value(facts, k) {
            Value::Array(a) => format!("{k}: list of {}", a.len()),
            Value::Object(_) => format!("{k}: object"),
            v => format!("{k}: {v}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The prompt for one pass; `rejected` says why the last spec was turned down.
pub fn prompt(app: &App, facts: &Value, rejected: Option<&str>) -> String {
    let owner = if app.cfg.owner.trim().is_empty() { "the owner".to_string() } else { app.cfg.owner.trim().to_string() };
    let span = if facts["kind"] == "monthly" { "month" } else { "week" };
    let period = &facts["period"];
    let mut s = format!(
        "You lay out the {span}ly recap page {owner} reads about their AI agents' work for the {span} {} to {}. \
         Pick components from the catalog below, in order, and write a one-sentence summary and up to {MAX_HIGHLIGHTS} short highlights.\n\n\
         Rules:\n\
         - Never write a number, a date or a count yourself, not even in words (\"three\", \"a dozen\", \"twice\"). Every number in text is a {{key}} placeholder for a fact key from the list below that holds a number, such as {{totals.tasks}}; the board fills it in, with units (minutes show as hours). Write plain words around it: \"{{totals.tasks}} tasks finished\". A list's length is its `.count` key, such as {{projects.count}}.\n\
         - Goal, project, task and release names may be quoted exactly as the facts give them. Refs (G12, T340, R7) may be used only when they're in the facts.\n\
         - Props point at facts by key or at a goal by ref; they never hold values. A fact key goes in `fact` ({{\"name\": \"a\", \"fact\": \"totals.added\"}}), and only a goal's ref goes in `ref` ({{\"name\": \"goal\", \"ref\": \"G12\"}}). Leave a prop out to use its default key.\n\
         - Each component's `why` says in a few plain words why it's on this page.\n\
         - Choose by what stands out in this {span} so different periods get different pages: lead with the period's real story (one big goal, a run of releases, a quiet stretch, an ordinary busy {span}). Don't fall back to the same layout every time.\n\
         - Use 5 to 9 components. Start with an opener (open, hero or heroR), then the lead story, then support. Left-out components give their place to the next one that fits.\n\
         - Write for {owner}: short plain sentences, no jargon, no praise or filler. Agent time is agent-hours added across tasks working side by side, so call it agent time, never hours on the clock. Estimated human time is an estimate; say so.\n\n\
         Page types, as a guide rather than a rule (judge which story this {span} tells):\n\
         - big: the top goal is 40% or more of the estimated human time or the tasks. Opener open; lead spot, story, vs; support kpis, time, fact.\n\
         - release: 15 or more releases in a week, or 40 in a month. Opener heroR; lead timeline, first, gcards; support kpis, board, time, fact.\n\
         - quiet: tasks under half of the previous period. Opener open; lead quiet, days, wait; support ba, list, time, lessons.\n\
         - normal: anything else. Opener hero; lead gcards, days, time; support board, ba, vs, fact.\n",
        period["first_day"].as_str().unwrap_or_default(),
        period["last_day"].as_str().unwrap_or_default(),
    );
    if span == "month" {
        s += "- A month also shows its week-by-week trend, its plan quality and its usage when they fit.\n";
    }
    s += &format!("\nWhat stands out:\n- {}\n", signals(facts).join("\n- "));
    s += &format!("\nAbout the data:\n- {}\n", data_notes(app).join("\n- "));
    s += &format!("\n{}\n", catalog_text(facts));
    s += &format!("What the numbers mean:\n{}\n\n", serde_json::to_string_pretty(&facts["labels"]).unwrap_or_default());
    s += &format!("Fact keys (key: value):\n{}\n\n", key_list(facts));
    s += &format!("The facts (task lists cut short):\n{}\n", jdumps(&prompt_facts(facts)));
    if let Some(why) = rejected {
        s += &format!("\nYour last page was turned down: {why}. Fix that and answer again.\n");
    }
    s
}

// ------------------------------------------------------------------ making and keeping a page

fn facts_of(app: &App, id: i64) -> Result<Value> {
    let Some(r) = app.db.q1("SELECT facts FROM recaps WHERE id = ?", p![id])? else { return err(404, "There's no recap with that id.") };
    Ok(r.s("facts").and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null))
}

/// Makes recap `id`'s page and keeps it: `ask` answers a prompt with a spec, tried up to [`TRIES`]
/// times; without it, or when every try fails, the page is the [`default_page`].
pub fn compose(app: &App, id: i64, ask: Option<&dyn Fn(String) -> std::result::Result<Value, String>>) -> Result<Value> {
    let facts = facts_of(app, id)?;
    let mut attempts = Vec::new();
    let mut made = None;
    if let Some(ask) = ask {
        let mut rejected: Option<String> = None;
        for _ in 0..TRIES {
            let (spec, result) = match ask(prompt(app, &facts, rejected.as_deref())) {
                Ok(spec) => {
                    let page = check(&spec, &facts);
                    (spec, page)
                }
                Err(e) => (Value::Null, Err(e)),
            };
            match result {
                Ok(page) => {
                    made = Some((spec, page));
                    break;
                }
                Err(e) => {
                    attempts.push(json!({"at": now_iso(), "error": e, "spec": spec}));
                    rejected = Some(e);
                }
            }
        }
    }
    let writing = match made {
        Some((spec, page)) => json!({"page_version": PAGE_VERSION, "source": "ai", "made_at": now_iso(), "attempts": attempts, "spec": spec, "page": page}),
        None => json!({
            "page_version": PAGE_VERSION, "source": "default", "made_at": now_iso(), "attempts": attempts, "spec": Value::Null,
            "why": if ask.is_none() { "Claude isn't available to the board" } else { "no spec from Claude passed the checks" },
            "page": default_page(&facts),
        }),
    };
    app.db.x("UPDATE recaps SET writing = ?, writing_status = 'done' WHERE id = ?", p![jdumps(&writing), id])?;
    Ok(writing)
}

/// Writes recap `id`'s page: on the AI queue when Claude is there to ask, otherwise the default
/// page now. A page already being written is left to finish.
pub fn queue(app: &App, id: i64) -> Result<()> {
    if !triage::ai_on(app) {
        compose(app, id, None)?;
        return Ok(());
    }
    if app.db.q1("SELECT 1 AS x FROM recaps WHERE id = ? AND writing_status = 'writing'", p![id])?.is_some() {
        return Ok(());
    }
    app.db.x("UPDATE recaps SET writing_status = 'writing' WHERE id = ?", p![id])?;
    app.queue_ai(Box::new(move |a: &App| {
        let ask = |prompt: String| {
            triage::ask_model_limited(a, &a.cfg.backlog.model, prompt, schema(), None, &[], TIMEOUT_SECS, BUDGET_USD, None)
        };
        if let Err(e) = compose(a, id, Some(&ask)) {
            a.info(format!("recap page: recap {id} failed: {}", e.message));
            let _ = a.db.x("UPDATE recaps SET writing_status = 'failed' WHERE id = ?", p![id]);
        }
    }));
    Ok(())
}
