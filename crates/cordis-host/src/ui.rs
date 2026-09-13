//! Materializes validated guest descriptions as retained native GPUI controls.
//! Paint and text editing use this local state; only actions enter the controller.
use crate::controller::{Command, Update, Views};
use cordis_core::Generation;
use cordis_wire::{Action, Node};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    component::{
        button::Button,
        input::{Input, InputEvent, InputState},
        *,
    },
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc,
    time::Duration,
};

struct NativeInput {
    generation: Generation,
    entity: Entity<InputState>,
    revision: u64,
    _subscription: Subscription,
}
pub(crate) struct NativeApp {
    views: Views,
    inputs: BTreeMap<String, NativeInput>,
    send: mpsc::SyncSender<Command>,
    status: String,
    last_error: Option<String>,
}
impl NativeApp {
    pub(crate) fn new(
        send: mpsc::SyncSender<Command>,
        receive: mpsc::Receiver<Update>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(16)).await;
                let mut updates = Vec::new();
                while let Ok(update) = receive.try_recv() {
                    updates.push(update);
                }
                if !updates.is_empty()
                    && this
                        .update(cx, |this, cx| {
                            for update in updates {
                                this.apply_update(update);
                            }
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            views: vec![],
            inputs: BTreeMap::new(),
            send,
            status: "Loading Wasm components…".into(),
            last_error: None,
        }
    }
    fn apply_update(&mut self, update: Update) {
        if let Some(views) = update.views {
            let mut input_keys = BTreeSet::new();
            for (plugin, _, node) in &views {
                collect_input_keys(plugin, node, &mut input_keys);
            }
            // Dropping removed inputs also drops their event subscriptions.
            self.inputs.retain(|key, _| input_keys.contains(key));
            self.views = views;
        }
        if !update.status.starts_with("Ready")
            && (update.status.contains("failed") || update.status.contains("error"))
        {
            self.last_error = Some(update.status.chars().take(1200).collect());
        }
        self.status = update
            .status
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
    }

    fn send_action(
        &mut self,
        plugin: &str,
        generation: Generation,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let prefix = format!("{plugin}/");
        let inputs: BTreeMap<_, _> = self
            .inputs
            .iter()
            .filter_map(|(key, input)| {
                let local_key = key.strip_prefix(&prefix)?;
                Some((
                    local_key.to_string(),
                    input.entity.read(cx).value().to_string(),
                ))
            })
            .collect();
        let payload = serde_json::json!({
            "inputs": inputs,
            "today": chrono::Local::now().format("%Y-%m-%d").to_string(),
        });
        let revision = self
            .inputs
            .values()
            .map(|input| input.revision)
            .max()
            .unwrap_or(0);
        let action = Action {
            id: id.into(),
            value: Some(payload.to_string()),
            revision,
        };
        if self
            .send
            .try_send(Command::Action(generation, action))
            .is_err()
        {
            self.status = "Action queue full".into();
            cx.notify();
        }
    }

    fn materialize(
        &mut self,
        plugin: &str,
        generation: Generation,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            Node::Column { children, .. } => div()
                .flex()
                .flex_col()
                .gap_3()
                .children(
                    children
                        .iter()
                        .map(|n| self.materialize(plugin, generation, n, window, cx))
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
            Node::Row { children, .. } => div()
                .flex()
                .gap_3()
                .items_center()
                .children(
                    children
                        .iter()
                        .map(|n| self.materialize(plugin, generation, n, window, cx))
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
            Node::Text { key, text } => div()
                .id(SharedString::from(format!("{plugin}/{key}")))
                .role(accesskit::Role::Label)
                .aria_label(text.clone())
                .flex_1()
                .child(text.clone())
                .into_any_element(),
            Node::Input {
                key, label, value, ..
            } => {
                let identity = format!("{plugin}/{key}");
                if !self
                    .inputs
                    .get(&identity)
                    .is_some_and(|entry| entry.generation == generation)
                {
                    // Explicit v1 view payload: restore plain text into a fresh native
                    // entity. No GPUI entity or guest handle crosses generations.
                    let initial = self
                        .inputs
                        .get(&identity)
                        .map(|entry| entry.entity.read(cx).value().to_string())
                        .unwrap_or_else(|| value.clone());
                    let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
                    let subscription_key = identity.clone();
                    let subscription = cx.subscribe_in(
                        &input,
                        window,
                        move |this, _, event: &InputEvent, _, _| {
                            if matches!(event, InputEvent::Change)
                                && let Some(entry) = this.inputs.get_mut(&subscription_key)
                            {
                                entry.revision += 1;
                            }
                        },
                    );
                    self.inputs.insert(
                        identity.clone(),
                        NativeInput {
                            generation,
                            entity: input,
                            revision: 0,
                            _subscription: subscription,
                        },
                    );
                }
                // A description supplies the initial value. Existing native edit/IME
                // state is authoritative; ordinary snapshots never set_value on it.
                let input = self.inputs[&identity].entity.clone();
                Input::new(&input)
                    .aria_label(label.clone())
                    .accessibility_id(identity)
                    .into_any_element()
            }
            Node::Button {
                key,
                label,
                action,
                name,
            } => {
                let action = action.clone();
                let plugin = plugin.to_string();
                Button::new(SharedString::from(format!("{plugin}/{key}")))
                    .flex_1()
                    .label(label.clone())
                    .accessibility_label(name.clone().unwrap_or_else(|| label.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send_action(&plugin, generation, &action, cx);
                    }))
                    .into_any_element()
            }
        }
    }
}
impl Render for NativeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let views = self.views.clone();
        let mut panels = Vec::new();
        for (id, generation, node) in views {
            let reload_id = id.clone();
            panels.push(
                div()
                    .flex_1()
                    .p_4()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded_lg()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                Button::new(SharedString::from(format!("reload-{id}")))
                                    .label(format!("Reload {id}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let _ =
                                            this.send.try_send(Command::Reload(reload_id.clone()));
                                        this.status = "Preparing replacement…".into();
                                        cx.notify();
                                    })),
                            )
                            .child(self.materialize(&id, generation, &node, window, cx)),
                    ),
            );
        }
        div()
            .id("workspace")
            .size_full()
            .overflow_y_scroll()
            .p_5()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child("Native plugin harness — GPUI + Wasm")
                    .child(
                        div()
                            .id("runtime-status")
                            .role(accesskit::Role::Label)
                            .aria_label(self.status.clone())
                            .child(self.status.clone()),
                    )
                    .child(
                        Button::new("native-repaint")
                            .label("Repaint native snapshot (no guest call)")
                            .on_click(cx.listener(|_, _, _, cx| cx.notify())),
                    )
                    .when_some(self.last_error.clone(), |this, error| {
                        this.child(
                            div()
                                .id("last-error")
                                .role(accesskit::Role::Label)
                                .aria_label(error.clone())
                                .text_sm()
                                .child(error),
                        )
                    })
                    .child(div().flex().gap_4().children(panels)),
            )
    }
}

fn collect_input_keys(plugin: &str, node: &Node, keys: &mut BTreeSet<String>) {
    match node {
        Node::Input { key, .. } => {
            keys.insert(format!("{plugin}/{key}"));
        }
        Node::Column { children, .. } | Node::Row { children, .. } => {
            for child in children {
                collect_input_keys(plugin, child, keys);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
