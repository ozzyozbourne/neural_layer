wit_bindgen::generate!({path:"../../wit", world:"plugin"});
use chrono::{Datelike, Duration, NaiveDate};
use cordis_wire::{Action, Issue, Node};
use poc::harness::host;
use std::sync::Mutex;
// Guest-local navigation resets when a fresh plugin generation activates.
struct CalendarState {
    month_offset: i32,
    selected_day: Option<String>,
}
static VIEW: Mutex<CalendarState> = Mutex::new(CalendarState {
    month_offset: 0,
    selected_day: None,
});
struct Calendar;
impl Guest for Calendar {
    fn activate(_config: String) -> Result<(), String> {
        host::acquire("lease")?;
        Ok(())
    }
    fn deactivate() -> Result<(), String> {
        Ok(())
    }
    fn dispatch(method: String, payload: String) -> Result<String, String> {
        match method.as_str() {
            "refresh" => host::call("issues", "list", "{}"),
            "diagnostic-delay" => host::call("issues", "delay", "{}"),
            "bad-payload" => host::call("issues", "list", "true"),
            "unknown-method" => host::call("issues", "unknown", "{}"),
            "read-private" => host::storage("scan", "{}"),
            "forward" => {
                let call: serde_json::Value =
                    serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                host::call(
                    call["service"].as_str().ok_or("service")?,
                    call["method"].as_str().ok_or("method")?,
                    call["payload"].as_str().ok_or("payload")?,
                )
            }
            _ => Err("unknown method".into()),
        }
    }

    fn render(input: String) -> Result<String, String> {
        let context: serde_json::Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        let issues: Vec<Issue> =
            serde_json::from_value(context["data"].clone()).map_err(|e| e.to_string())?;
        let today = NaiveDate::parse_from_str(
            context["today"].as_str().ok_or("today missing")?,
            "%Y-%m-%d",
        )
        .map_err(|e| e.to_string())?;
        let (offset, selected) = {
            let state = VIEW.lock().unwrap();
            (state.month_offset, state.selected_day.clone())
        };
        let month_index = today.year() * 12 + today.month0() as i32 + offset;
        let first = NaiveDate::from_ymd_opt(
            month_index.div_euclid(12),
            month_index.rem_euclid(12) as u32 + 1,
            1,
        )
        .ok_or("month out of range")?;
        let start = first - Duration::days(first.weekday().num_days_from_monday() as i64);
        let mut children = vec![
            Node::Text {
                key: "heading".into(),
                text: format!("{} · completed issues", first.format("%B %Y")),
            },
            Node::Row {
                key: "navigation".into(),
                children: vec![
                    Node::Button {
                        name: None,
                        key: "prev".into(),
                        label: "Previous month".into(),
                        action: "prev".into(),
                    },
                    Node::Button {
                        name: None,
                        key: "next".into(),
                        label: "Next month".into(),
                        action: "next".into(),
                    },
                ],
            },
            Node::Row {
                key: "weekdays".into(),
                children: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                    .into_iter()
                    .map(|day| Node::Text {
                        key: format!("weekday-{day}"),
                        text: day.into(),
                    })
                    .collect(),
            },
        ];
        for week in 0..6 {
            let mut cells = Vec::new();
            for weekday in 0..7 {
                let date = start + Duration::days(week * 7 + weekday);
                let day = date.to_string();
                let count = issues
                    .iter()
                    .filter(|i| i.completed_day.as_deref() == Some(&day))
                    .count();
                cells.push(Node::Button {
                    name: Some(format!(
                        "{}, {count} completed issues",
                        date.format("%A, %B %e, %Y")
                    )),
                    key: format!("day-{day}"),
                    label: if count == 0 {
                        format!("{}", date.day())
                    } else {
                        format!("{} · {count}", date.day())
                    },
                    action: format!("day:{day}"),
                });
            }
            children.push(Node::Row {
                key: format!("week-{week}"),
                children: cells,
            });
        }
        if let Some(day) = selected {
            children.push(Node::Text {
                key: "selected".into(),
                text: format!("Completed on {day}"),
            });
            for issue in issues
                .iter()
                .filter(|i| i.completed_day.as_deref() == Some(&day))
            {
                children.push(Node::Text {
                    key: format!("selected-{}", issue.id),
                    text: issue.title.clone(),
                });
            }
        }
        serde_json::to_string(&Node::Column {
            key: "calendar".into(),
            children,
        })
        .map_err(|e| e.to_string())
    }
    fn handle_action(input: String) -> Result<(), String> {
        let action: Action = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        let mut state = VIEW.lock().unwrap();
        match action.id.as_str() {
            "prev" => state.month_offset -= 1,
            "next" => state.month_offset += 1,
            id if id.starts_with("day:") => state.selected_day = Some(id[4..].into()),
            _ => return Err("unknown action".into()),
        };
        Ok(())
    }
}
export!(Calendar);
