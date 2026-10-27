//! A published assembly must support ordinary metadata navigation under the shared budget.
use super::*;

#[test]
fn dotnet_metadata_budget_deferred_call_projection_keeps_generic_contexts_independent() {
    // resolve_call is a public library operation, not used by the current CLI's
    // ILSpy path. Exercise that seam directly rather than expose a test-only flag.
    let mut parsed = rxd_dotnet::Input::read(&input(), rxd_dotnet::Limits::default()).unwrap();
    parsed.resolve_method_token(0x06000021).unwrap();
    parsed.resolve_method_token(0x06000022).unwrap();
    parsed.resolve_method_token(0x06000024).unwrap();
    let generic = parsed.method_by_id("0x06000022").unwrap();
    let mixed = parsed.method_by_id("0x06000024").unwrap();
    for (context, token, returns) in [
        (generic, 0x2b000001, "!!1"),
        (mixed, 0x2b000002, "!!0"),
        (generic, 0x2b000001, "!!1"),
    ] {
        let call = parsed.resolve_call(token, context).unwrap();
        assert_eq!(call.token, token);
        assert_eq!(call.parameters.as_slice(), ["!!1"]);
        assert_eq!(call.return_type.as_str(), returns);
    }
    let add = parsed.resolve_call(0x06000001, generic).unwrap();
    assert_eq!(add.parameters.as_slice(), ["int32", "int32"]);
    assert_eq!(add.return_type.as_str(), "int32");
    assert_eq!(
        parsed
            .method_by_id("0x06000021")
            .unwrap()
            .return_type
            .as_str(),
        "!!0"
    );
    assert!(parsed.resolve_call(0x0a000002, generic).is_err());

    // A deferred projection cannot reset the work already consumed by metadata.
    // Find the public read boundary rather than bake in platform-dependent costs.
    let (mut low, mut high) = (0, rxd_dotnet::Limits::default().work.unwrap());
    while high - low > 1 {
        let work = low + (high - low) / 2;
        let limits = rxd_dotnet::Limits {
            work: Some(work),
            ..rxd_dotnet::Limits::default()
        };
        match rxd_dotnet::Input::read(&input(), limits) {
            Ok(_) => high = work,
            Err(error) => {
                assert!(error.to_string().contains("RESOURCE_LIMIT"), "{error:#}");
                low = work;
            }
        }
    }
    let limits = rxd_dotnet::Limits {
        work: Some(high),
        ..rxd_dotnet::Limits::default()
    };
    let exhausted = rxd_dotnet::Input::read(&input(), limits).unwrap();
    let context = exhausted.method_by_id("0x06000022").unwrap();
    let error = exhausted.resolve_call(0x06000001, context).unwrap_err();
    assert!(error.to_string().contains("RESOURCE_LIMIT"), "{error:#}");
}
