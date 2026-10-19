#[cfg(not(feature = "web"))]
use rxd_hook_gen::web;
use rxd_hook_gen::{CapturedInput, ErrorKind, Limits, Request, dex, generate};

#[test]
#[cfg(not(feature = "web"))]
fn generation_reports_unavailable_provider_without_an_artifact() {
    let request = Request::Web(web::Request {
        target: "globalThis.fetch".into(),
        ..Default::default()
    });
    let error = generate(&request, Limits::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::ProviderUnavailable);
    assert_eq!(error.exit_code(), 1);
    assert_eq!(error.target.as_deref(), Some("globalThis.fetch"));
}

#[test]
fn generation_rejects_oversized_selector_before_provider_analysis() {
    let limits = Limits {
        target_bytes: 4,
        ..Limits::default()
    };
    let request = Request::Dex(dex::Request {
        input: CapturedInput::from_bytes("fixture.dex", b"dex\n035\0".to_vec(), limits).unwrap(),
        selector: dex::Selector::Signature("LExample;->method()V".into()),
        options: Default::default(),
    });
    let error = generate(&request, limits).unwrap_err();
    assert_eq!(error.kind, ErrorKind::ResourceLimit);
    assert_eq!(error.exit_code(), 1);
}
