use super::*;
use core::prelude::v1::test;
#[gpui_kit::test]
fn stale_snapshot_preserves_native_ime_and_reload_restores_text(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let description = Node::Input {
        key: "title".into(),
        label: "Title".into(),
        value: String::new(),
        revision: 0,
        action: "edit".into(),
    };
    let (send, _) = mpsc::sync_channel(1);
    let handle = cx.add_window(|_, _| NativeApp {
        views: vec![("test".into(), 1, description.clone())],
        inputs: BTreeMap::new(),
        send,
        status: String::new(),
        last_error: None,
    });
    // Materialize through a real GPUI draw, so elements belong to the App's
    // frame arena and are released at the same boundary as in the application.
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let first = handle
        .update(cx, |view, window, cx| {
            let input = view.inputs["test/title"].entity.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
                assert_eq!(input.marked_text_range(window, cx), Some(0..2));
            });
            view.views = vec![("test".into(), 1, description.clone())];
            cx.notify();
            input
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    handle
        .update(cx, |view, window, cx| {
            assert_eq!(first, view.inputs["test/title"].entity);
            first.update(cx, |input, cx| {
                assert_eq!(input.value(), "ni");
                assert_eq!(input.marked_text_range(window, cx), Some(0..2));
                input.replace_text_in_range(None, "你", window, cx);
                assert_eq!(input.value(), "你");
            });
            view.views = vec![("test".into(), 2, description.clone())];
            cx.notify();
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    handle
        .update(cx, |view, _, cx| {
            assert_ne!(first, view.inputs["test/title"].entity);
            assert_eq!(view.inputs["test/title"].entity.read(cx).value(), "你");
        })
        .unwrap();
    drop(first);
    cx.run_until_parked();
}
