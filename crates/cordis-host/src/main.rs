//! Native application startup. Plugin orchestration and UI have separate owners.
mod controller;
mod ui;

use anyhow::Result;
use cordis_host::storage::Database;
use gpui_kit::{component::Root, *};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};
use ui::NativeApp;

gpui_kit::actions!(harness, [Quit]);

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::create_dir_all(root.join("data"))?;
    let storage = Arc::new(Database::open(&root.join("data/issues.sqlite"))?);
    let (send, commands) = mpsc::sync_channel(64);
    let (updates, receive) = mpsc::channel();
    let controller = send.clone();
    std::thread::spawn(move || controller::run(root, storage, controller, commands, updates));
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            gpui_kit::profiler::set_trace_enabled(true);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.activate(true);
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(80.), px(80.)),
                            size(px(1100.), px(720.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view = cx.new(|cx| NativeApp::new(send, receive, cx));
                        cx.new(|cx| Root::new(view, window, cx))
                    },
                )
                .expect("open native window");
            })
            .detach();
        });
    Ok(())
}
