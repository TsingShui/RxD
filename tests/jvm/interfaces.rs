use super::*;

#[test]
fn rhino_interfaces_render_declarations_and_superinterfaces() {
    let input = std::path::Path::new("tests/corpus/jvm/rhino-1.7.15.jar");
    for (at, declaration, method) in [
        (
            "Lorg/mozilla/javascript/ClassShutter;",
            "public interface ClassShutter {",
            "visibleToScripts",
        ),
        (
            "Lorg/mozilla/javascript/Function;",
            "public interface Function extends org.mozilla.javascript.Scriptable, org.mozilla.javascript.Callable, org.mozilla.javascript.Constructable {",
            "construct",
        ),
    ] {
        let source = text(run(input, "get-class", &["--at", at]));
        assert!(source.contains(declaration), "{source}");
        assert!(!source.contains(" implements "), "{source}");
        assert!(source.contains(method), "{source}");
    }
}
