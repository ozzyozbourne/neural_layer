wit_bindgen::generate!({path:"../../wit", world:"plugin"});
use cordis_wire::{Action, Issue, Node};
use poc::harness::host;
struct Issues;
impl Guest for Issues {
    fn activate(config: String) -> Result<(), String> {
        host::acquire("pool")?;
        host::storage("scan", "{}")?;
        if config == "trap-after-acquire" {
            panic!("activation fault injection");
        }

        Ok(())
    }
    fn deactivate() -> Result<(), String> {
        Ok(())
    }
    fn dispatch(method: String, _payload: String) -> Result<String, String> {
        match method.as_str() {
            "list" => host::storage("scan", "{}"),
            "delay" => {
                host::pause(1500)?;
                Ok("null".into())
            }
            "count" => {
                let issues: Vec<Issue> = serde_json::from_str(&host::storage("scan", "{}")?)
                    .map_err(|e| e.to_string())?;
                Ok(issues.len().to_string())
            }
            _ => Err("unknown method".into()),
        }
    }
    fn render(input: String) -> Result<String, String> {
        // Render is pure: the host supplies application data from a prior query.
        let context: serde_json::Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        let issues: Vec<Issue> =
            serde_json::from_value(context["data"].clone()).map_err(|e| e.to_string())?;
        let mut children = vec![
            Node::Text {
                key: "heading".into(),
                text: "Issues · Wasm plugin".into(),
            },
            Node::Input {
                key: "new-title".into(),
                label: "New issue title".into(),
                value: String::new(),
                revision: 0,
                action: "draft".into(),
            },
            Node::Button {
                name: None,
                key: "add".into(),
                label: "Create issue".into(),
                action: "create".into(),
            },
        ];
        children.push(Node::Row {
            key: "diagnostics".into(),
            children: vec![
                Node::Button {
                    name: None,
                    key: "delay".into(),
                    label: "Delay guest 1.5s".into(),
                    action: "delay".into(),
                },
                Node::Button {
                    name: None,
                    key: "loop".into(),
                    label: "Test guest budget".into(),
                    action: "loop".into(),
                },
            ],
        });
        for issue in issues {
            children.push(Node::Column {
                key: format!("issue-{}", issue.id),
                children: vec![
                    Node::Input {
                        key: format!("title-{}", issue.id),
                        label: format!("Issue {} title", issue.id),
                        value: issue.title,
                        revision: 0,
                        action: "draft".into(),
                    },
                    Node::Button {
                        name: None,
                        key: format!("save-{}", issue.id),
                        label: "Save title".into(),
                        action: format!("save:{}", issue.id),
                    },
                    Node::Button {
                        name: None,
                        key: format!("delete-{}", issue.id),
                        label: "Delete issue".into(),
                        action: format!("delete:{}", issue.id),
                    },
                    Node::Button {
                        name: None,
                        key: format!("status-{}", issue.id),
                        label: if issue.completed_day.is_some() {
                            "Reopen"
                        } else {
                            "Complete today"
                        }
                        .into(),
                        action: format!("toggle:{}", issue.id),
                    },
                ],
            });
        }
        serde_json::to_string(&Node::Column {
            key: "issues".into(),
            children,
        })
        .map_err(|e| e.to_string())
    }
    fn handle_action(encoded: String) -> Result<(), String> {
        let action: Action = serde_json::from_str(&encoded).map_err(|e| e.to_string())?;
        let context: serde_json::Value =
            serde_json::from_str(action.value.as_deref().unwrap_or("{}"))
                .map_err(|e| e.to_string())?;
        if let Some(id) = action.id.strip_prefix("delete:") {
            let id = id.parse::<i64>().map_err(|e| e.to_string())?;
            host::storage(
                "batch",
                &serde_json::json!([{"key":id.to_string(),"delete":true}]).to_string(),
            )?;
        } else if action.id == "create"
            || action.id.starts_with("toggle:")
            || action.id.starts_with("save:")
        {
            let mut issues: Vec<Issue> =
                serde_json::from_str(&host::storage("scan", "{}")?).map_err(|e| e.to_string())?;
            let issue = if action.id == "create" {
                let title = context["inputs"]["new-title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if title.trim().is_empty() {
                    return Err("title is required".into());
                }
                Issue {
                    id: issues.iter().map(|i| i.id).max().unwrap_or(0) + 1,
                    title,
                    completed_day: None,
                }
            } else {
                let id = action
                    .id
                    .split_once(':')
                    .ok_or("invalid action")?
                    .1
                    .parse::<i64>()
                    .map_err(|e| e.to_string())?;
                let issue = issues
                    .iter_mut()
                    .find(|i| i.id == id)
                    .ok_or("issue not found")?;
                if action.id.starts_with("save:") {
                    let title = context["inputs"][format!("title-{id}")]
                        .as_str()
                        .ok_or("title required")?;
                    if title.trim().is_empty() {
                        return Err("title is required".into());
                    }
                    issue.title = title.into();
                } else {
                    issue.completed_day = if issue.completed_day.is_some() {
                        None
                    } else {
                        Some(
                            context["today"]
                                .as_str()
                                .ok_or("current date required")?
                                .to_string(),
                        )
                    };
                }
                issue.clone()
            };
            host::storage(
                "batch",
                &serde_json::json!([{"key":issue.id.to_string(),"value":issue}]).to_string(),
            )?;
        } else if action.id == "delay" {
            host::pause(1500)?;
        } else if action.id == "loop" {
            loop {
                std::hint::spin_loop();
            }
        } else if action.id != "draft" {
            return Err("unknown action".into());
        }
        Ok(())
    }
}
export!(Issues);
