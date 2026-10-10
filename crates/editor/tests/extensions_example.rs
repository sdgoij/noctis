//! The worked example under `examples/extensions` is kept honest: it loads, its
//! verb edits the scene, and the whole edit is one undo step.

use std::path::Path;

use editor::{Editor, ExtensionHost};
use extensions::Loader;

#[test]
fn the_scatter_example_loads_and_scatters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions");
    let loader = Loader::discover(&[root]);
    assert!(loader.errors().is_empty(), "{:?}", loader.errors());
    assert_eq!(loader.ids(), ["com.example.scatter"]);

    let mut host = ExtensionHost::new().expect("the extension host starts");
    host.load(&loader, |_| true);
    assert!(host.errors().is_empty(), "{:?}", host.errors());

    let mut editor = Editor::new();
    let source = editor.spawn("Source", None);
    editor.select(source).expect("select the source");

    let reply = host.dispatch(&mut editor, "scatter 4").expect("dispatch");
    assert!(reply.starts_with("ok scatter"), "unexpected reply: {reply}");
    assert_eq!(
        editor.list_entities().len(),
        5,
        "the source plus four copies"
    );

    // The whole scatter is one undo step.
    assert!(editor.can_undo());
    editor.undo();
    assert_eq!(editor.list_entities().len(), 1);
}
